use crate::utils::{AllowOnlyOne, AllowOnlyOneError};
use bytecodec::{Decode, Encode, EncodeExt, Eos};
use futures::future;
use futures::stream;
use futures::stream::{FuturesUnordered, Stream, StreamExt};
use std::fmt::{self, Debug, Display, Formatter};
use std::net::SocketAddr;
use stun_codec::rfc5389;
use stun_codec::rfc5389::Attribute;
use stun_codec::{
    BrokenMessage, Message, MessageClass, MessageDecoder, MessageEncoder, TransactionId,
};
use tokio::net::{lookup_host, UdpSocket};

const STUN_URLS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun2.l.google.com:19302",
    "stun3.l.google.com:19302",
    "stun4.l.google.com:19302",
];

fn stun_servers() -> impl Stream<Item = SocketAddr> {
    let dns_res: FuturesUnordered<_> = STUN_URLS.iter().map(lookup_host).collect();
    let addr_iter = dns_res
        .map(|res| res.ok().into_iter().flatten())
        .map(stream::iter)
        .flatten();
    addr_iter.filter(|addr| future::ready(addr.is_ipv4()))
}

fn create_msg() -> Result<impl AsRef<[u8]>, bytecodec::Error> {
    let id_bytes = [0xFF; 12];
    let id = TransactionId::new(id_bytes);
    let msg = Message::<Attribute>::new(MessageClass::Request, rfc5389::methods::BINDING, id);
    let mut enc = MessageEncoder::with_item(msg)?;
    let mut buf = [0; 20];
    let written = enc.encode(&mut buf, Eos::new(true))?;
    debug_assert_eq!(written, buf.len());
    Ok(buf)
}

pub enum StunMappingError {
    Io(tokio::io::Error),
    Codec(bytecodec::Error),
    BrokenMessage(BrokenMessageError),
    MultipleBindings,
}
impl Debug for StunMappingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            StunMappingError::Io(inner) => Debug::fmt(inner, f),
            StunMappingError::Codec(inner) => Debug::fmt(inner, f),
            StunMappingError::BrokenMessage(inner) => Debug::fmt(inner, f),
            StunMappingError::MultipleBindings => write!(f, "StunMappingError::MultipleBindings"),
        }
    }
}

impl Display for StunMappingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            StunMappingError::Io(inner) => Display::fmt(inner, f),
            StunMappingError::Codec(inner) => Display::fmt(inner, f),
            StunMappingError::BrokenMessage(inner) => Display::fmt(inner, f),
            StunMappingError::MultipleBindings => {
                write!(f, "Multiple addresses returned from STUN.")
            }
        }
    }
}

impl std::error::Error for StunMappingError {}

impl From<tokio::io::Error> for StunMappingError {
    fn from(inner: tokio::io::Error) -> Self {
        Self::Io(inner)
    }
}
impl From<bytecodec::Error> for StunMappingError {
    fn from(inner: bytecodec::Error) -> Self {
        Self::Codec(inner)
    }
}

impl From<BrokenMessage> for StunMappingError {
    fn from(inner: BrokenMessage) -> Self {
        Self::BrokenMessage(BrokenMessageError::from(inner))
    }
}

impl From<AllowOnlyOneError<SocketAddr>> for StunMappingError {
    fn from(_: AllowOnlyOneError<SocketAddr>) -> Self {
        Self::MultipleBindings
    }
}

async fn try_stun(
    socket: &mut UdpSocket,
    server: &SocketAddr,
) -> Result<Option<SocketAddr>, StunMappingError> {
    let msg = create_msg()?;
    socket.send_to(msg.as_ref(), server).await?;
    let mut mbuff = [0; 32];
    let (_got, from) = socket.recv_from(&mut mbuff).await?;
    debug_assert_eq!(&from, server);
    let mut decoder = MessageDecoder::<Attribute>::new();
    let _declen = decoder.decode(&mbuff, Eos::new(true))?;
    let dec: Message<Attribute> = decoder.finish_decoding()??;
    let addrs: AllowOnlyOne<_> = dec.attributes().filter_map(attribute_to_addr).collect();
    let retvl = match addrs.into_res() {
        Ok(a) => Ok(Some(a)),
        Err(AllowOnlyOneError::GotSecond(_)) => Err(StunMappingError::MultipleBindings),
        Err(AllowOnlyOneError::NoneFound) => Ok(None),
    };
    retvl
}

fn attribute_to_addr(attr: &Attribute) -> Option<SocketAddr> {
    let addr = match attr {
        Attribute::MappedAddress(mapped) => Some(mapped.address()),
        Attribute::XorMappedAddress(mapped) => Some(mapped.address()),
        Attribute::XorMappedAddress2(mapped) => Some(mapped.address()),
        _ => None,
    };
    addr
}

pub struct BrokenMessageError(BrokenMessage);

impl Display for BrokenMessageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "BrokenMessageError with source ID 0x")?;
        let id = self.0.transaction_id();
        let id_bytes = id.as_bytes();
        for byte in id_bytes {
            write!(f, "{:02X}", byte)?;
        }
        write!(f, " : {}", self.0.error())?;
        Ok(())
    }
}

impl Debug for BrokenMessageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrokenMessageError")
            .field("MessageClass", &self.0.class())
            .field("MessageMethod", &self.0.method())
            .field("MessageId", &self.0.transaction_id())
            .field("Error", &self.0.error())
            .finish()
    }
}
impl std::error::Error for BrokenMessageError {}

impl From<BrokenMessage> for BrokenMessageError {
    fn from(inner: BrokenMessage) -> Self {
        Self(inner)
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct StunMapping {
    local_addr: SocketAddr,
    public_addr: SocketAddr,
}

impl StunMapping {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
    pub fn public_addr(&self) -> SocketAddr {
        self.public_addr
    }
    pub async fn get_mapping(socket: &mut UdpSocket) -> Result<Option<Self>, StunMappingError> {
        let server_iter = stun_servers();
        futures::pin_mut!(server_iter);
        let mut previous_addr: StunMappingError =
            StunMappingError::Io(std::io::ErrorKind::Other {}.into());
        while let Some(addr) = server_iter.next().await {
            let pubres = try_stun(socket, &addr).await;
            match pubres {
                Ok(Some(public_addr)) => {
                    let local_addr = socket.local_addr()?;
                    return Ok(Some(StunMapping {
                        local_addr,
                        public_addr,
                    }));
                }
                Ok(None) => {
                    return Ok(None);
                }
                Err(e) => previous_addr = e.into(),
            }
        }
        Err(previous_addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    #[tokio::test]
    async fn test1() {
        let mut socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 36709))
            .await
            .unwrap();
        println!("{:?}", socket.local_addr().unwrap());
        let addr = super::StunMapping::get_mapping(&mut socket)
            .await
            .unwrap()
            .unwrap();
        println!("{:?}", addr);
    }
}
