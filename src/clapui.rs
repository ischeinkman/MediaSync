use crate::communication::FriendCode;
use crate::Args;
use clap;
use clap::{App, Arg, ArgMatches};
use std::path::Path;

pub fn init_parser<'a, 'b>() -> App<'a, 'b> {
    App::new("vlcsync")
        .version("test")
        .arg(
            Arg::with_name("public")
                .long("public")
                .short("p")
                .multiple(false)
                .takes_value(false)
                .help("Try to open a public port."),
        )
        .arg(
            Arg::with_name("debug")
                .long("debug")
                .multiple(false)
                .takes_value(false)
                .help("Show debug player options."),
        )
        .arg(
            Arg::with_name("connections")
                .value_name("friend code")
                .short("c")
                .long("connect")
                .takes_value(true)
                .multiple(true)
                .validator(|s| {
                    let s = s.trim();
                    if s.len() != 9 {
                        return Err(format!(
                            "Friend codes have length {} but value {} has length {}",
                            9,
                            s,
                            s.len()
                        ));
                    }
                    for c in s.chars() {
                        if !c.is_alphanumeric() {
                            return Err(format!("Invalid char {}.", c));
                        }
                    }
                    Ok(())
                })
                .help("Connect to a given friend code on start."),
        )
        .arg(
            Arg::with_name("mpvsockets")
                .value_name("socket path")
                .long("mpvsock")
                .takes_value(true)
                .multiple(true)
                .validator(|s| {
                    let s = s.trim();
                    let cur_path = &Path::new(s);
                    if !cur_path.is_file() {
                        Err(format!("File not found at path {}", s))
                    } else {
                        Ok(())
                    }
                })
                .help("Search for an mpv ipc instance at the given file name."),
        )
}

pub fn map_args(parsed: ArgMatches<'_>) -> Args {
    let mut retvl = Args::default();
    let connections = parsed
        .values_of("connections")
        .into_iter()
        .flat_map(|v| v)
        .map(|arg| FriendCode::from_code(arg).unwrap())
        .collect();
    retvl.connect_to = connections;
    retvl.open_public = parsed.is_present("public");
    retvl.show_debug = parsed.is_present("debug");
    retvl.mpv_socket = parsed
        .values_of("mpvsockets")
        .into_iter()
        .flat_map(|v| v)
        .map(|s| s.to_owned())
        .collect();
    retvl
}

/*
use std::time::{Duration, Instant, SystemTime};
const SECONDS_IN_MINUTE: u64 = 60;
const MINUTES_IN_HOUR: u64 = 60;
const HOURS_IN_DAY: u64 = 24;
const SECONDS_IN_HOUR: u64 = MINUTES_IN_HOUR * SECONDS_IN_MINUTE;
const SECONDS_IN_DAY: u64 = SECONDS_IN_HOUR * HOURS_IN_DAY;
const DAYS_IN_WEEK: u64 = 7;

const SECONDS_IN_YEAR: u64 = (365 * SECONDS_IN_DAY) + SECONDS_IN_DAY / 4;

const DAYS_PER_MONTH_NONLEAP: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const LAST_YEAR_DAY_PER_MONTH_NONLEAP: [u64; 12] =
    [31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334, 365];
const DAYS_PER_MONTH_LEAP: [u64; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const LAST_YEAR_DAY_PER_MONTH_LEAP: [u64; 12] =
    [31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335, 366];

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
struct FormattedTime {
    year: u64,
    month: u64,
    day_in_month: u64,
    day_in_week: u64,
    hour: u64,
    minute: u64,
    second: u64,
}

impl PartialOrd for FormattedTime {
    fn partial_cmp(&self, other: &FormattedTime) -> Option<std::cmp::Ordering> {
        Some(self.cmp(&other))
    }
}

impl Ord for FormattedTime {
    fn cmp(&self, other: &FormattedTime) -> std::cmp::Ordering {
        if self.year != other.year {
            self.year.cmp(&other.year)
        } else if self.month != other.month {
            self.month.cmp(&other.month)
        } else if self.day_in_month != other.day_in_month {
            self.day_in_month.cmp(&other.day_in_month)
        } else if self.hour != other.hour {
            self.hour.cmp(&other.hour)
        } else if self.minute != other.minute {
            self.minute.cmp(&other.minute)
        } else if self.second != other.second {
            self.second.cmp(&other.second)
        } else {
            std::cmp::Ordering::Equal
        }
    }
}

impl FormattedTime {
    pub fn from_unix_secs(secs: u64) -> FormattedTime {
        const UNIX_ZERO_AS_GREGORIAN_SECONDS: u64 = 1970 * SECONDS_IN_YEAR;
        const UNIX_ZERO_DAY_IN_WEEK: u64 = 4; //Thursday

        let gregorian_secs = secs + UNIX_ZERO_AS_GREGORIAN_SECONDS;
        let year = gregorian_secs / SECONDS_IN_YEAR;
        let days = secs / SECONDS_IN_DAY;

        let day_in_week = (days + 4) % 7;
        let hour = (secs / SECONDS_IN_HOUR) % HOURS_IN_DAY;
        let minute = (secs / SECONDS_IN_MINUTE) % MINUTES_IN_HOUR;
        let second = secs % 60;
        unimplemented!()
    }
}

const fn one_if_leap(year: u64) -> u64 {
    let leap_offset = year % 4;
    (4 - leap_offset) / 4
}
const fn days_in_year(year: u64) -> u64 {
    365 + one_if_leap(year)
}
const fn days_in_month_for_year(year: u64) -> [u64; 12] {
    [DAYS_PER_MONTH_NONLEAP, DAYS_PER_MONTH_LEAP][one_if_leap(year) as usize]
}
const fn month_borders_for_year(year: u64) -> [u64; 12] {
    [
        LAST_YEAR_DAY_PER_MONTH_NONLEAP,
        LAST_YEAR_DAY_PER_MONTH_LEAP,
    ][one_if_leap(year) as usize]
}*/
