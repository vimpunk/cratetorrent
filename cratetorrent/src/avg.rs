// there are some APIs that are not being used at the moment but are going to be
// used when new features are added
#![allow(dead_code)]

use std::{convert::TryInto, time::Duration};

/// This is an exponential moving average accumulator.
///
/// An algorithm is used that addresss the initial bias that occurs when all
/// values are initialized with zero or with the first sample (which would bias
/// the average toward the first value). This is achieved by initially giving
/// a low gain for the average and slowly increasing it until the inverted gain
/// is reached.
///
/// For example, the first sample should have a gain of 1 as the average has no
/// meaning.  When adding the second sample, the average has some meaning, but
/// since it only has one sample in it, the gain should be low. In the next
/// round however, the gain may be larger. This increase is repeated until
/// inverted gain is reached.  This way, even early samples have a reasonable
/// impact on the average, which is important in a torrent app.
///
/// Ported from libtorrent: https://blog.libtorrent.org/2014/09/running-averages/
#[derive(Debug)]
pub struct SlidingAvg {
    /// The current running average, effectively the mean.
    ///
    /// This is a fixed-point value, that is, the sample is multiplied by 64
    /// before adding it. When the mean is returned, 32 is added and the sum is
    /// divided back by 64, to eliminate integer truncation that would result in
    /// a bias.
    // TODO: can we just use float for the calculations? although this is likely
    // faster
    mean: u64,
    /// The average deviation.
    ///
    /// This is a fixed-point value, that is, the sample is multiplied by 64
    /// before adding it. When the mean is returned, 32 is added and the sum is
    /// divided back by 64, to eliminate integer truncation that would result in
    /// a bias.
    deviation: u64,
    /// The number of samples received, but no more than `inverted_gain`.
    sample_count: usize,
    /// This is the threshold used for determining how many initial samples to
    /// give a higher gain than the current average.
    // TODO: turn this into a const generic parameter once that's supported
    inverted_gain: usize,
}

impl SlidingAvg {
    pub fn new(inverted_gain: usize) -> Self {
        Self {
            mean: 0,
            deviation: 0,
            sample_count: 0,
            inverted_gain,
        }
    }

    pub fn update(&mut self, mut sample: u64) {
        // see comment in `Self::mean`
        sample *= 64;

        let deviation = if self.sample_count > 0 {
            ((self.mean - sample) as i64).abs() as u64
        } else {
            0
        };

        if self.sample_count < self.inverted_gain {
            self.sample_count += 1;
        }

        self.mean += (sample - self.mean) / self.sample_count as u64;

        if self.sample_count > 1 {
            self.deviation +=
                (deviation - self.deviation) / (self.sample_count - 1) as u64;
        }
    }

    pub fn mean(&self) -> u64 {
        if self.sample_count == 0 {
            0
        } else {
            (self.mean + 32) / 64
        }
    }

    pub fn deviation(&self) -> u64 {
        if self.sample_count == 0 {
            0
        } else {
            (self.deviation + 32) / 64
        }
    }
}

impl Default for SlidingAvg {
    /// Creates a sliding average with an inverted gain of 20.
    fn default() -> Self {
        Self::new(20)
    }
}

/// Wraps a [`SlidingAvg`] instance and converts the statistics to
/// [`std::time::Duration`] units (keeping everything in the underlying layer as
/// milliseconds).
#[derive(Debug)]
pub struct SlidingDurationAvg(SlidingAvg);

impl SlidingDurationAvg {
    pub fn new(inverted_gain: usize) -> Self {
        Self(SlidingAvg::new(inverted_gain))
    }

    pub fn update(&mut self, sample: Duration) {
        // TODO: is this safe? Duration::from_millis takes u64 but as_millis
        // returns u128 so it's not clear
        let ms = sample.as_millis().try_into().expect("Millisecond overflow");
        self.0.update(ms);
    }

    pub fn mean(&self) -> Duration {
        let ms = self.0.mean();
        Duration::from_millis(ms)
    }

    pub fn deviation(&self) -> Duration {
        let ms = self.0.deviation();
        Duration::from_millis(ms)
    }
}

impl Default for SlidingDurationAvg {
    /// Creates a sliding average with an inverted gain of 20.
    fn default() -> Self {
        Self(SlidingAvg::default())
    }
}
