use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::net::UdpSocket;

pub async fn random_localaddr(min_port: u16, max_port: u16) -> Result<SocketAddr, std::io::Error> {
    let ip = local_network_ip().await?;
    let port = random_port(min_port, max_port);

    let addr = SocketAddr::from((ip, port));
    Ok(addr)
}

pub async fn local_network_ip() -> Result<IpAddr, std::io::Error> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 40000)).await?;
    socket.connect((Ipv4Addr::new(8, 8, 8, 8), 4000)).await?;
    let got_addr = socket.local_addr()?;
    Ok(got_addr.ip())
}

pub fn random_port(from: u16, to: u16) -> u16 {
    let valid_range = to - from;
    let info: u16 = rand::random();
    let offset = info % valid_range;
    from + offset
}
