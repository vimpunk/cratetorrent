use std::fmt;

pub use Unit::*;

/// A convenience type for pretty-printing units of computer storage
/// measurement.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub enum Unit {
    Bytes(u64),
    KiB(f64),
    MiB(f64),
    GiB(f64),
}

impl Unit {
    pub fn new(bytes: u64) -> Self {
        let mut r = bytes as f64;
        let mut depth = 0;

        while r > 1024.0 && depth < 3 {
            r /= 1024.0;
            depth += 1;
        }

        match depth {
            0 => Self::Bytes(bytes),
            1 => Self::KiB(r),
            2 => Self::MiB(r),
            3 => Self::GiB(r),
            _ => unreachable!(),
        }
    }
}

impl From<u64> for Unit {
    fn from(bytes: u64) -> Self {
        Self::new(bytes)
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Bytes(b) => write!(f, "{} B", b),
            KiB(b) => write!(f, "{:.2} KiB", b),
            MiB(b) => write!(f, "{:.2} MiB", b),
            GiB(b) => write!(f, "{:.2} GiB", b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_convert_not_convert_bytes() {
        let unit = Unit::new(1011);
        match unit {
            Bytes(u) => {
                assert_eq!(u, 1011);
            }
            _ => {
                assert!(false, "unit should remain in bytes");
            }
        }
    }

    #[test]
    fn should_convert_bytes_to_kib() {
        let unit = Unit::new(5_460);
        // we should ideally test at least the first 2 decimal places too, but
        // this should do for the time being...
        match unit {
            KiB(u) => {
                assert_eq!(u.trunc(), 5.0);
            }
            _ => {
                assert!(false, "unit should be in KiB");
            }
        }
    }

    #[test]
    fn should_convert_bytes_to_mib() {
        let unit = Unit::new(32_123_962);
        match unit {
            MiB(u) => {
                assert_eq!(u.trunc(), 30.0);
            }
            _ => {
                assert!(false, "unit should be in MiB");
            }
        }
    }

    #[test]
    fn should_convert_bytes_to_gib() {
        let unit = Unit::new(2_229_863_925);
        match unit {
            GiB(u) => {
                assert_eq!(u.trunc(), 2.0);
            }
            _ => {
                assert!(false, "unit should be in GiB");
            }
        }

        let unit = Unit::new(12_262_129_863_925);
        match unit {
            GiB(u) => {
                assert_eq!(u.trunc(), 11_419.0);
            }
            _ => {
                assert!(false, "unit should be in GiB");
            }
        }
    }
}
