//! d3-style scales and tick generation.

/// "Nice" step (1, 2, 5 x 10^k) giving about `count` ticks over `span`.
pub fn nice_step(span: f64, count: usize) -> f64 {
    if !(span > 0.0) || count == 0 {
        return 1.0;
    }
    let raw = span / count as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let nice = if norm < 1.5 {
        1.0
    } else if norm < 3.0 {
        2.0
    } else if norm < 7.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// Nice tick values inside `[lo, hi]`.
pub fn nice_ticks(lo: f64, hi: f64, count: usize) -> Vec<f64> {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    if !lo.is_finite() || !hi.is_finite() {
        return Vec::new();
    }
    let step = nice_step(hi - lo, count);
    // tolerate float error at the domain ends (0.3 / 0.1 = 2.9999999999999996)
    let first = (lo / step - 1e-9).ceil() as i64;
    let last = (hi / step + 1e-9).floor() as i64;
    if last < first || last - first > 10_000 {
        return Vec::new();
    }
    // divide by the inverse step for sub-unit steps (d3 trick): 3 / 10 = 0.3 exactly
    let inv = 1.0 / step;
    let exact_inv = step < 1.0 && (inv - inv.round()).abs() < 1e-9;
    (first..=last)
        .map(|i| {
            if exact_inv {
                i as f64 / inv.round()
            } else {
                i as f64 * step
            }
        })
        .collect()
}

/// Continuous linear scale `domain -> range`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearScale {
    pub domain: (f64, f64),
    pub range: (f64, f64),
}

impl LinearScale {
    pub fn new(domain: (f64, f64), range: (f64, f64)) -> Self {
        Self { domain, range }
    }
    pub fn map(&self, v: f64) -> f64 {
        let t = (v - self.domain.0) / (self.domain.1 - self.domain.0);
        self.range.0 + t * (self.range.1 - self.range.0)
    }
    pub fn invert(&self, r: f64) -> f64 {
        let t = (r - self.range.0) / (self.range.1 - self.range.0);
        self.domain.0 + t * (self.domain.1 - self.domain.0)
    }
    pub fn ticks(&self, count: usize) -> Vec<f64> {
        nice_ticks(self.domain.0, self.domain.1, count)
    }
}

/// Base-10 log scale. The domain must be positive; `ticks` returns nothing
/// otherwise and `map` yields NaN/inf.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogScale {
    pub domain: (f64, f64),
    pub range: (f64, f64),
}

impl LogScale {
    pub fn new(domain: (f64, f64), range: (f64, f64)) -> Self {
        Self { domain, range }
    }
    pub fn map(&self, v: f64) -> f64 {
        let (a, b) = (self.domain.0.log10(), self.domain.1.log10());
        self.range.0 + (v.log10() - a) / (b - a) * (self.range.1 - self.range.0)
    }
    pub fn invert(&self, r: f64) -> f64 {
        let (a, b) = (self.domain.0.log10(), self.domain.1.log10());
        10f64.powf(a + (r - self.range.0) / (self.range.1 - self.range.0) * (b - a))
    }
    /// Powers of ten plus 2x and 5x subdivisions when the span is small.
    pub fn ticks(&self) -> Vec<f64> {
        let (lo, hi) = (
            self.domain.0.min(self.domain.1),
            self.domain.0.max(self.domain.1),
        );
        if !(lo > 0.0) || !hi.is_finite() {
            return Vec::new(); // log scale needs a positive, finite domain
        }
        let (e0, e1) = (lo.log10().floor() as i32, hi.log10().ceil() as i32);
        let subs: &[f64] = if e1 - e0 <= 3 {
            &[1.0, 2.0, 5.0]
        } else {
            &[1.0]
        };
        let mut out = Vec::new();
        for e in e0..=e1 {
            for s in subs {
                let v = s * 10f64.powi(e);
                if v >= lo && v <= hi {
                    out.push(v);
                }
            }
        }
        out
    }
}

/// Ordinal/band scale for bar charts: `n` categories over a pixel range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandScale {
    pub n: usize,
    pub range: (f64, f64),
    /// Fraction of each band left empty (0..1).
    pub padding: f64,
}

impl BandScale {
    pub fn step(&self) -> f64 {
        (self.range.1 - self.range.0) / self.n.max(1) as f64
    }
    pub fn bandwidth(&self) -> f64 {
        self.step() * (1.0 - self.padding)
    }
    /// Start of band `i`.
    pub fn map(&self, i: usize) -> f64 {
        self.range.0 + self.step() * i as f64 + self.step() * self.padding / 2.0
    }
}

/// A time tick: timestamp in ms and whether it is the first tick in a new
/// larger calendar unit (hour for sub-hour steps, day for intraday steps,
/// month for day steps, year for longer steps), useful for bold labels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeTick {
    pub time_ms: i64,
    pub major: bool,
}

/// Time scale over Unix-millisecond timestamps (UTC).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeScale {
    pub domain: (i64, i64),
}

const SEC: i64 = 1_000;
const MIN: i64 = 60 * SEC;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;

impl TimeScale {
    /// Ticks aligned to round calendar units (approximately `count`).
    /// Months/years use 30/365-day steps aligned to UTC days, a
    /// simplification documented as future work.
    pub fn ticks(&self, count: usize) -> (i64, Vec<TimeTick>) {
        const STEPS: &[i64] = &[
            SEC,
            5 * SEC,
            15 * SEC,
            30 * SEC,
            MIN,
            5 * MIN,
            15 * MIN,
            30 * MIN,
            HOUR,
            3 * HOUR,
            6 * HOUR,
            12 * HOUR,
            DAY,
            2 * DAY,
            7 * DAY,
            30 * DAY,
            90 * DAY,
            365 * DAY,
        ];
        let (lo, hi) = (
            self.domain.0.min(self.domain.1),
            self.domain.0.max(self.domain.1),
        );
        let target = (hi - lo) / count.max(1) as i64;
        let step = STEPS
            .iter()
            .copied()
            .find(|s| *s >= target)
            .unwrap_or_else(|| {
                // round *up* to a nice number of years so we never exceed `count`
                let years = (target as f64 / (365 * DAY) as f64).ceil().max(1.0);
                let nice = nice_step(years, 1).max(1.0);
                let nice = if nice < years { nice * 2.0 } else { nice };
                nice as i64 * 365 * DAY
            });
        // unit whose change marks a tick as major
        let unit = |t: i64| -> i64 {
            let (y, m, d, h) = civil(t);
            if step < HOUR {
                (y * 12 + m) * 31 * 24 + d * 24 + h
            } else if step < DAY {
                (y * 12 + m) * 31 + d
            } else if step < 30 * DAY {
                y * 12 + m
            } else {
                y
            }
        };
        let mut t = lo.div_euclid(step) * step;
        if t < lo {
            t += step;
        }
        let mut out = Vec::new();
        let mut prev = unit(t - step);
        while t <= hi && out.len() < 10_000 {
            let u = unit(t);
            out.push(TimeTick {
                time_ms: t,
                major: u != prev,
            });
            prev = u;
            t += step;
        }
        (step, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_ticks_basic() {
        assert_eq!(
            nice_ticks(0.0, 10.0, 5),
            vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]
        );
        assert_eq!(nice_ticks(0.0, 0.3, 3), vec![0.0, 0.1, 0.2, 0.3]);
        assert_eq!(
            nice_ticks(0.0, 1.2, 6),
            vec![0.0, 0.2, 0.4, 0.6, 0.8, 1.0, 1.2]
        );
        let t = nice_ticks(101.3, 104.9, 4);
        assert_eq!(t, vec![102.0, 103.0, 104.0]);
    }

    #[test]
    fn linear_roundtrip() {
        let s = LinearScale::new((10.0, 20.0), (600.0, 0.0));
        assert_eq!(s.map(10.0), 600.0);
        assert!((s.invert(s.map(13.7)) - 13.7).abs() < 1e-9);
    }

    #[test]
    fn log_ticks() {
        let s = LogScale::new((1.0, 1000.0), (0.0, 300.0));
        assert!((s.map(10.0) - 100.0).abs() < 1e-9);
        assert!(s.ticks().contains(&100.0));
        assert!(LogScale::new((0.0, 1000.0), (0.0, 1.0)).ticks().is_empty());
        assert!(LogScale::new((-5.0, 10.0), (0.0, 1.0)).ticks().is_empty());
    }

    #[test]
    fn time_ticks_hourly() {
        let s = TimeScale {
            domain: (0, 10 * HOUR),
        };
        let (step, ticks) = s.ticks(10);
        assert_eq!(step, HOUR);
        assert_eq!(ticks.len(), 11);
        assert!(ticks[0].major);
        assert!(!ticks[1].major);
    }

    #[test]
    fn time_ticks_multi_year_never_exceed_count() {
        for years in [5i64, 20, 39, 100] {
            let s = TimeScale {
                domain: (0, years * 365 * DAY),
            };
            let (_, ticks) = s.ticks(10);
            assert!(ticks.len() <= 11, "{years}y -> {} ticks", ticks.len());
        }
    }
}

/// UTC civil date/time of a Unix-ms timestamp: (year, month, day, hour).
fn civil(ms: i64) -> (i64, i64, i64, i64) {
    let secs = ms.div_euclid(1000);
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    (y, m, d, sod / 3600)
}

/// Howard Hinnant's civil_from_days.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Format a Unix-ms timestamp as UTC `YYYY-MM-DD HH:MM` (no deps).
pub fn format_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        sod / 3600,
        sod % 3600 / 60
    )
}

#[cfg(test)]
mod fmt_tests {
    #[test]
    fn utc() {
        assert_eq!(super::format_utc(0), "1970-01-01 00:00");
        assert_eq!(super::format_utc(1_600_000_000_000), "2020-09-13 12:26");
    }
}
