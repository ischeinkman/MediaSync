fn decode_ipv4(digits: [char; 6]) -> u32 {
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
            0
        };
        retvl += coeff * scaled;
    }

    retvl
}

#[allow(unused)]
fn decode_ipv6(digits: [char; 22]) -> u128 {
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

    retvl
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

use std::net::SocketAddrV4;

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

fn decode_socketaddrv4(code: [char; 9]) -> SocketAddrV4 {
    let ip_bytes = [code[0], code[1], code[2], code[3], code[4], code[5]];
    let ip_num = decode_ipv4(ip_bytes);
    let port_bytes = [code[6], code[7], code[8]];
    let port = decode_port(port_bytes);
    SocketAddrV4::new(ip_num.into(), port)
}

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct FriendCodeV4 {
    addr: SocketAddrV4,
}

impl FriendCodeV4 {
    pub fn from_code(code: [char; 9]) -> Self {
        FriendCodeV4 {
            addr: decode_socketaddrv4(code),
        }
    }
    pub fn from_addr(addr: SocketAddrV4) -> Self {
        FriendCodeV4 { addr }
    }
    pub fn as_friend_code(self) -> [char; 9] {
        encode_socketaddrv4(self.addr)
    }
    pub fn as_addr(self) -> SocketAddrV4 {
        self.addr
    }
}

impl From<SocketAddrV4> for FriendCodeV4 {
    fn from(addr: SocketAddrV4) -> FriendCodeV4 {
        FriendCodeV4::from_addr(addr)
    }
}

impl From<[char; 9]> for FriendCodeV4 {
    fn from(code: [char; 9]) -> FriendCodeV4 {
        FriendCodeV4::from_code(code)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::net::Ipv4Addr;
    #[test]
    fn test_port_codec() {
        let port = 41234;
        assert_eq!(port, decode_port(encode_port(port)));
    }

    #[test]
    fn test_ipv4_codec() {
        let ips: &[Ipv4Addr] = &[
            Ipv4Addr::new(192, 168, 1, 32),
            Ipv4Addr::BROADCAST,
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::new(8, 8, 8, 8),
        ];
        for &ip in ips {
            let ip = ip.into();
            assert_eq!(ip, decode_ipv4(encode_ipv4(ip)));
        }
    }
}
