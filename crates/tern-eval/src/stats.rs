//! Small statistics: means, 95% intervals over trials, Wilson intervals
//! for proportions.

/// The mean, or 0 for nothing.
pub fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

/// The sample standard deviation (n − 1), or 0 with fewer than two values.
pub fn sd(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (xs.len() - 1) as f64).sqrt()
}

/// Student's t for a two-sided 95% interval with `df` degrees of freedom.
pub fn t95(df: usize) -> f64 {
    const T: [f64; 30] = [
        12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262, 2.228, 2.201, 2.179,
        2.160, 2.145, 2.131, 2.120, 2.110, 2.101, 2.093, 2.086, 2.080, 2.074, 2.069, 2.064, 2.060,
        2.056, 2.052, 2.048, 2.045, 2.042,
    ];
    match df {
        0 => f64::INFINITY,
        d if d <= 30 => T[d - 1],
        _ => 1.96,
    }
}

/// A 95% interval for the mean of `xs` (one value per trial): mean ± t·s/√n.
/// With one value the interval is that value.
pub fn t_interval(xs: &[f64]) -> (f64, f64) {
    let m = mean(xs);
    if xs.len() < 2 {
        return (m, m);
    }
    let half = t95(xs.len() - 1) * sd(xs) / (xs.len() as f64).sqrt();
    (m - half, m + half)
}

/// The Wilson score 95% interval for `k` successes in `n`.
pub fn wilson(k: u64, n: u64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let z = 1.959_964_f64;
    let n = n as f64;
    let p = k as f64 / n;
    let denom = 1.0 + z * z / n;
    let centre = (p + z * z / (2.0 * n)) / denom;
    let half = z * ((p * (1.0 - p) / n) + z * z / (4.0 * n * n)).sqrt() / denom;
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilson_matches_known_values() {
        // 8 of 10: 0.490 to 0.943 (Wilson 1927; any stats table).
        let (lo, hi) = wilson(8, 10);
        assert!((lo - 0.490).abs() < 0.002, "{lo}");
        assert!((hi - 0.943).abs() < 0.002, "{hi}");
        assert_eq!(wilson(0, 0), (0.0, 1.0));
        let (lo, hi) = wilson(10, 10);
        assert!(lo > 0.69 && (hi - 1.0).abs() < 1e-9);
    }

    #[test]
    fn t_interval_widens_with_spread() {
        assert_eq!(t_interval(&[0.5, 0.5, 0.5]), (0.5, 0.5));
        let (lo, hi) = t_interval(&[0.4, 0.5, 0.6]);
        // sd 0.1, n 3, t 4.303: half width 0.2484.
        assert!((hi - lo - 2.0 * 0.2484).abs() < 0.001, "{lo} {hi}");
        assert_eq!(t_interval(&[0.7]), (0.7, 0.7));
    }
}
