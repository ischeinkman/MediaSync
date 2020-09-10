use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};

fn decode_ipv4(digits: [char; 6]) -> Result<u32, FriendCodeError> {
    let mut retvl = 0;
    for (idx, &c) in digits.iter().enumerate() {
        let power = 5 - (idx as u32);
        let scaled = 62u32.pow(power);
        let coeff = if c >= '0' && c <= '9' {
            c as u32 - '0' as u32
        } else if c >= 'a' && c <= 'z' {
            c as u32 - 'a' as u32 + 10
        } else if c >= 'A' && c <= 'Z' {
            c as u32 - 'A' as u32 + 36
        } else {
            return Err(FriendCodeError::InvalidCharacter(c));
        };
        retvl += coeff * scaled;
    }

    Ok(retvl)
}

fn decode_ipv6(digits: [char; 22]) -> Result<u128, FriendCodeError> {
    let mut retvl = 0u128;
    for (idx, &c) in digits.iter().enumerate() {
        let power = 21 - (idx as u32);
        let scaled = 62u128.pow(power);
        let coeff = if c >= '0' && c <= '9' {
            c as u128 - '0' as u128
        } else if c >= 'a' && c <= 'z' {
            c as u128 - 'a' as u128 + 10
        } else if c >= 'A' && c <= 'Z' {
            c as u128 - 'A' as u128 + 36
        } else {
            0
        };
        retvl += coeff * scaled;
    }

    Ok(retvl)
}

fn decode_port(digits: [char; 3]) -> u16 {
    let mut retvl = 0u16;
    for (idx, &c) in digits.iter().enumerate() {
        let power = 2 - (idx as u32);
        let scaled = 62u16.pow(power);
        let coeff = if c >= '0' && c <= '9' {
            c as u16 - '0' as u16
        } else if c >= 'a' && c <= 'z' {
            c as u16 - 'a' as u16 + 10
        } else if c >= 'A' && c <= 'Z' {
            c as u16 - 'A' as u16 + 36
        } else {
            0
        };
        retvl += coeff * scaled;
    }
    retvl
}

fn encode_port(port: u16) -> [char; 3] {
    let mut retvl = ['\0'; 3];
    let mut left = u32::from(port);
    let mut cur_idx = 2;
    while left > 0 {
        let digit = left % 62;
        let digit_char = if digit < 10 {
            std::char::from_digit(digit, 10).unwrap()
        } else if digit < 36 {
            let offset = digit - 10 + u32::from('a');
            std::char::from_u32(offset).unwrap()
        } else {
            let offset = digit - 36 + u32::from('A');
            std::char::from_u32(offset).unwrap()
        };
        retvl[cur_idx] = digit_char;
        if left < 62 {
            break;
        }
        cur_idx -= 1;
        left /= 62;
    }

    retvl
}

fn encode_ipv4(ip: u32) -> [char; 6] {
    let mut retvl = ['\0'; 6];
    let mut left = ip;
    let mut cur_idx = 5;
    while left > 0 {
        let digit = left % 62;
        let digit_char = if digit < 10 {
            std::char::from_digit(digit, 10).unwrap()
        } else if digit < 36 {
            let offset = digit - 10 + u32::from('a');
            std::char::from_u32(offset).unwrap()
        } else {
            let offset = digit - 36 + u32::from('A');
            std::char::from_u32(offset).unwrap()
        };
        retvl[cur_idx] = digit_char;
        if left < 62 {
            break;
        }
        left /= 62;
        cur_idx -= 1;
    }

    retvl
}

fn encode_ipv6(ip: u128) -> [char; 22] {
    let mut retvl = ['\0'; 22];
    let mut left = ip;
    let mut cur_idx = 21;
    while left > 0 {
        let digit = (left % 62) as u32;
        let digit_char = if digit < 10 {
            std::char::from_digit(digit, 10).unwrap()
        } else if digit < 36 {
            let offset = digit - 10 + u32::from('a');
            std::char::from_u32(offset).unwrap()
        } else {
            let offset = digit - 36 + u32::from('A');
            std::char::from_u32(offset).unwrap()
        };
        retvl[cur_idx] = digit_char;
        if left < 62 {
            break;
        }
        left /= 62;
        cur_idx -= 1;
    }

    retvl
}

fn encode_socketaddrv4(addr: impl Into<SocketAddrV4>) -> [char; 9] {
    let addr = addr.into();
    let ip_bytes = (*addr.ip()).into();
    let encoded_ip = encode_ipv4(ip_bytes);
    let port_bytes = addr.port();
    let encoded_port = encode_port(port_bytes);
    [
        encoded_ip[0],
        encoded_ip[1],
        encoded_ip[2],
        encoded_ip[3],
        encoded_ip[4],
        encoded_ip[5],
        encoded_port[0],
        encoded_port[1],
        encoded_port[2],
    ]
}

fn encode_socketaddrv6(addr: impl Into<SocketAddrV6>) -> [char; 25] {
    let addr = addr.into();
    let ip_bytes = (*addr.ip()).into();
    let encoded_ip = encode_ipv6(ip_bytes);
    let port_bytes = addr.port();
    let encoded_port = encode_port(port_bytes);
    let mut retvl = ['\0'; 25];
    (&mut retvl[..22]).copy_from_slice(&encoded_ip);
    (&mut retvl[22..]).copy_from_slice(&encoded_port);
    retvl
}

fn decode_socketaddrv4(code: [char; 9]) -> Result<SocketAddrV4, FriendCodeError> {
    let ip_bytes = [code[0], code[1], code[2], code[3], code[4], code[5]];
    let ip_num = decode_ipv4(ip_bytes)?;
    let port_bytes = [code[6], code[7], code[8]];
    let port = decode_port(port_bytes);
    Ok(SocketAddrV4::new(ip_num.into(), port))
}
fn decode_socketaddrv6(code: [char; 25]) -> Result<SocketAddrV6, FriendCodeError> {
    let mut ip_bytes = ['\0'; 22];
    (&mut ip_bytes).copy_from_slice(&code[..22]);
    let ip_num = decode_ipv6(ip_bytes)?;
    let port_bytes = [code[22], code[23], code[24]];
    let port = decode_port(port_bytes);
    Ok(SocketAddrV6::new(ip_num.into(), port, 0, 0))
}

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct FriendCode {
    addr: SocketAddr,
}
#[derive(Eq, PartialEq, Debug)]
pub enum FriendCodeError {
    InvalidLength(usize),
    InvalidCharacter(char),
}

impl FriendCode {
    pub fn from_code(code: impl AsRef<str>) -> Result<Self, FriendCodeError> {
        let code = code.as_ref();
        if code.len() == 25 {
            let mut buffer = ['\0'; 25];
            let code_chars = code.chars();
            for (out, byte) in buffer.iter_mut().zip(code_chars) {
                *out = byte;
            }
            FriendCode::from_code_v6(buffer)
        } else if code.len() == 9 {
            let mut buffer = ['\0'; 9];
            let code_chars = code.chars();
            for (out, byte) in buffer.iter_mut().zip(code_chars) {
                *out = byte;
            }
            FriendCode::from_code_v4(buffer)
        } else {
            Err(FriendCodeError::InvalidLength(code.len()))
        }
    }
    fn from_code_v4(code: [char; 9]) -> Result<Self, FriendCodeError> {
        decode_socketaddrv4(code).map(FriendCode::from_addr)
    }
    fn from_code_v6(code: [char; 25]) -> Result<Self, FriendCodeError> {
        decode_socketaddrv6(code).map(FriendCode::from_addr)
    }

    pub fn from_addr(addr: impl Into<SocketAddr>) -> Self {
        let addr = addr.into();
        Self { addr }
    }
    pub fn as_friend_code(self) -> String {
        match self.addr {
            SocketAddr::V4(addr) => {
                let bts = encode_socketaddrv4(addr);
                bts.iter().collect()
            }
            SocketAddr::V6(addr) => {
                let bts = encode_socketaddrv6(addr);
                bts.iter().collect()
            }
        }
    }
    pub fn as_addr(self) -> SocketAddr {
        self.addr
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const TRIALS: usize = 100;
    #[test]
    fn test_port_codec() {
        let port = 41234;
        assert_eq!(port, decode_port(encode_port(port)));
    }

    #[test]
    fn test_ipv4_codec() {
        for _ in 0..TRIALS {
            let ip = rand::random();
            assert_eq!(ip, decode_ipv4(encode_ipv4(ip)).unwrap());
        }
    }
    #[test]
    fn test_ipv6_codec() {
        for _ in 0..TRIALS {
            let ip = rand::random();
            assert_eq!(ip, decode_ipv6(encode_ipv6(ip)).unwrap());
        }
    }
}
