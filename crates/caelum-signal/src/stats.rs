//! Small robust statistics shared by the algorithms. All return `None` on empty input rather than
//! inventing a value.

/// Sorted copy (total order; inputs are finite where it matters).
pub(crate) fn sorted(x: &[f64]) -> Vec<f64> {
    let mut v = x.to_vec();
    v.sort_by(f64::total_cmp);
    v
}

/// Median.
pub(crate) fn median(x: &[f64]) -> Option<f64> {
    let v = sorted(x);
    let n = v.len();
    if n == 0 {
        return None;
    }
    let mid = n / 2;
    if n % 2 == 1 {
        v.get(mid).copied()
    } else {
        Some((v.get(mid - 1)? + v.get(mid)?) / 2.0)
    }
}

/// Median absolute deviation about the median (unscaled).
pub(crate) fn mad(x: &[f64], med: f64) -> Option<f64> {
    let dev: Vec<f64> = x.iter().map(|v| (v - med).abs()).collect();
    median(&dev)
}

/// Percentile `p` in `[0, 100]` with linear interpolation between order statistics.
pub(crate) fn percentile(x: &[f64], p: f64) -> Option<f64> {
    let v = sorted(x);
    percentile_sorted(&v, p)
}

/// Percentile of an already sorted slice.
pub(crate) fn percentile_sorted(v: &[f64], p: f64) -> Option<f64> {
    let n = v.len();
    if n == 0 || !p.is_finite() {
        return None;
    }
    let rank = (p.clamp(0.0, 100.0) / 100.0) * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let a = *v.get(lo)?;
    let b = *v.get(hi)?;
    Some(a + (b - a) * (rank - lo as f64))
}

/// Arithmetic mean.
pub(crate) fn mean(x: &[f64]) -> Option<f64> {
    if x.is_empty() {
        None
    } else {
        Some(x.iter().sum::<f64>() / x.len() as f64)
    }
}

/// Sample standard deviation (n − 1).
pub(crate) fn std_dev(x: &[f64]) -> Option<f64> {
    if x.len() < 2 {
        return None;
    }
    let m = mean(x)?;
    let ss: f64 = x.iter().map(|v| (v - m) * (v - m)).sum();
    Some((ss / (x.len() - 1) as f64).sqrt())
}

/// Root mean square.
pub(crate) fn rms(x: &[f64]) -> Option<f64> {
    if x.is_empty() {
        None
    } else {
        Some((x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt())
    }
}

/// Normalised autocorrelation of a zero-mean copy of `x` at `lag` (biased estimator).
pub(crate) fn autocorr(x: &[f64], lag: usize) -> Option<f64> {
    let m = mean(x)?;
    let var: f64 = x.iter().map(|v| (v - m) * (v - m)).sum();
    if var <= 0.0 || lag >= x.len() {
        return None;
    }
    let s: f64 = x
        .iter()
        .zip(x.iter().skip(lag))
        .map(|(a, b)| (a - m) * (b - m))
        .sum();
    Some(s / var)
}

/// Lag in `[min_lag, max_lag]` with the highest autocorrelation, refined by a parabola through
/// the neighbours. Returns `(lag, correlation)`; lags are in samples.
pub(crate) fn dominant_period(x: &[f64], min_lag: usize, max_lag: usize) -> Option<(f64, f64)> {
    let max_lag = max_lag.min(x.len().saturating_sub(2));
    if min_lag < 1 || min_lag >= max_lag {
        return None;
    }
    let acf: Vec<f64> = (min_lag - 1..=max_lag + 1)
        .map(|l| autocorr(x, l).unwrap_or(0.0))
        .collect();
    // Only true local maxima count: the edge of the search range is not a period.
    let mut best: Option<(usize, f64)> = None;
    for i in 1..acf.len() - 1 {
        let (a, b, c) = (acf[i - 1], acf[i], acf[i + 1]);
        if b > a && b >= c && best.is_none_or(|(_, v)| b > v) {
            best = Some((i, b));
        }
    }
    let (i, peak) = best?;
    let (a, c) = (acf[i - 1], acf[i + 1]);
    let denom = a - 2.0 * peak + c;
    let shift = if denom.abs() > f64::EPSILON {
        (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    Some(((min_lag - 1 + i) as f64 + shift, peak))
}

/// Pearson correlation of two equal-length series.
pub(crate) fn correlation(a: &[f64], b: &[f64]) -> Option<f64> {
    if a.len() != b.len() || a.len() < 2 {
        return None;
    }
    let ma = mean(a)?;
    let mb = mean(b)?;
    let mut sab = 0.0;
    let mut saa = 0.0;
    let mut sbb = 0.0;
    for (x, y) in a.iter().zip(b) {
        sab += (x - ma) * (y - mb);
        saa += (x - ma) * (x - ma);
        sbb += (y - mb) * (y - mb);
    }
    if saa <= 0.0 || sbb <= 0.0 {
        return None;
    }
    Some(sab / (saa * sbb).sqrt())
}

/// Linear interpolation of the points `(t, v)` (strictly increasing `t`) onto a uniform grid
/// starting at `t[0]` with step `1/fs`, up to the last point.
pub(crate) fn resample_uniform(t: &[f64], v: &[f64], fs: f64) -> Vec<f64> {
    let (Some(&t0), Some(&t_end)) = (t.first(), t.last()) else {
        return Vec::new();
    };
    if t.len() != v.len() || t.len() < 2 || fs <= 0.0 {
        return Vec::new();
    }
    let n = ((t_end - t0) * fs).floor() as usize + 1;
    let mut out = Vec::with_capacity(n);
    let mut j = 0;
    for k in 0..n {
        let tk = t0 + k as f64 / fs;
        while j + 2 < t.len() && t[j + 1] < tk {
            j += 1;
        }
        let (ta, tb, va, vb) = (t[j], t[j + 1], v[j], v[j + 1]);
        let w = if tb > ta { (tk - ta) / (tb - ta) } else { 0.0 };
        out.push(va + (vb - va) * w.clamp(0.0, 1.0));
    }
    out
}

/// Maps `x` linearly from `[good, bad]` onto a score `[1, 0]`, clamped.
pub(crate) fn ramp(x: f64, good: f64, bad: f64) -> f64 {
    if !x.is_finite() {
        return 0.0;
    }
    ((bad - x) / (bad - good)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robust_statistics() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(mad(&[1.0, 2.0, 3.0, 4.0, 100.0], 3.0), Some(1.0));
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 50.0), Some(3.0));
        assert_eq!(percentile(&[0.0, 10.0], 25.0), Some(2.5));
        assert_eq!(std_dev(&[1.0]), None);
        assert!((std_dev(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]).unwrap() - 2.138).abs() < 1e-3);
        assert_eq!(rms(&[3.0, -3.0]), Some(3.0));
        assert_eq!(ramp(0.5, 0.0, 1.0), 0.5);
        assert_eq!(ramp(f64::NAN, 0.0, 1.0), 0.0);
    }

    #[test]
    fn dominant_period_finds_a_sine_period() {
        let x: Vec<f64> = (0..1000)
            .map(|i| (2.0 * std::f64::consts::PI * i as f64 / 37.5).sin())
            .collect();
        let (lag, r) = dominant_period(&x, 10, 100).unwrap();
        assert!((lag - 37.5).abs() < 0.5, "{lag}");
        assert!(r > 0.9);
        assert_eq!(dominant_period(&[0.0; 50], 2, 20), None);
    }

    #[test]
    fn resampling_interpolates() {
        let out = resample_uniform(&[0.0, 1.0, 2.0], &[0.0, 10.0, 0.0], 4.0);
        assert_eq!(out.len(), 9);
        assert!((out[2] - 5.0).abs() < 1e-12);
        assert!((out[6] - 5.0).abs() < 1e-12);
        assert!(resample_uniform(&[0.0], &[1.0], 4.0).is_empty());
    }
}
