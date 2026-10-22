//! Turning scores into probabilities, and measuring how honest they are.

/// Softmax of `logits / temperature`, stable for large values.
pub fn softmax(logits: &[f32], temperature: f32) -> Vec<f32> {
    if logits.is_empty() {
        return Vec::new();
    }
    let t = temperature.max(1e-3);
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|l| ((l - max) / t).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|e| e / sum).collect()
}

/// Scales non-negative values to sum to 1; all zero becomes uniform.
pub fn normalize(values: &[f32]) -> Vec<f32> {
    let clean: Vec<f32> = values
        .iter()
        .map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 })
        .collect();
    let sum: f32 = clean.iter().sum();
    if sum <= 0.0 {
        let n = values.len().max(1) as f32;
        return vec![1.0 / n; values.len()];
    }
    clean.iter().map(|v| v / sum).collect()
}

/// 1 minus the normalised entropy: 1 when all the probability is on one
/// choice, 0 when it is spread evenly.
pub fn confidence(probs: &[f32]) -> f32 {
    if probs.len() < 2 {
        return 1.0;
    }
    let h: f32 = probs
        .iter()
        .filter(|p| **p > 0.0)
        .map(|p| -p * p.ln())
        .sum();
    (1.0 - h / (probs.len() as f32).ln()).clamp(0.0, 1.0)
}

/// Index of the largest value; the first on ties.
pub fn argmax(probs: &[f32]) -> usize {
    let mut best = 0;
    for (i, p) in probs.iter().enumerate() {
        if *p > probs[best] {
            best = i;
        }
    }
    best
}

/// Multi-class Brier score for one item (0 is perfect, 2 is worst).
pub fn brier(probs: &[f32], gold: usize) -> f32 {
    probs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let y = if i == gold { 1.0 } else { 0.0 };
            (p - y) * (p - y)
        })
        .sum()
}

/// Negative log probability of the right answer, floored at 1e-4.
pub fn log_loss(probs: &[f32], gold: usize) -> f32 {
    -probs.get(gold).copied().unwrap_or(0.0).max(1e-4).ln()
}

/// Expected calibration error over (confidence of the top choice, was it
/// right) pairs, in `bins` equal-width bins.
pub fn ece(items: &[(f32, bool)], bins: usize) -> f32 {
    if items.is_empty() || bins == 0 {
        return 0.0;
    }
    let mut total = 0.0;
    for b in 0..bins {
        let lo = b as f32 / bins as f32;
        let hi = (b + 1) as f32 / bins as f32;
        let inside: Vec<&(f32, bool)> = items
            .iter()
            .filter(|(p, _)| *p >= lo && (*p < hi || (b + 1 == bins && *p <= hi)))
            .collect();
        if inside.is_empty() {
            continue;
        }
        let conf: f32 = inside.iter().map(|(p, _)| p).sum::<f32>() / inside.len() as f32;
        let acc = inside.iter().filter(|(_, ok)| *ok).count() as f32 / inside.len() as f32;
        total += (conf - acc).abs() * inside.len() as f32 / items.len() as f32;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softmax_sums_to_one_and_keeps_order() {
        let p = softmax(&[1.0, 3.0, 2.0], 1.0);
        assert!((p.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert_eq!(argmax(&p), 1);
        let sharp = softmax(&[1.0, 3.0, 2.0], 0.1);
        assert!(sharp[1] > p[1]);
        assert!(softmax(&[1000.0, 0.0], 1.0)[0] > 0.99, "no overflow");
    }

    #[test]
    fn confidence_and_scores() {
        assert!(confidence(&[1.0, 0.0, 0.0]) > 0.99);
        assert!(confidence(&[0.25; 4]) < 0.01);
        assert_eq!(normalize(&[0.0, 0.0]), [0.5, 0.5]);
        assert_eq!(normalize(&[1.0, 3.0]), [0.25, 0.75]);
        assert!(brier(&[1.0, 0.0], 0) < 1e-6);
        assert!((brier(&[0.0, 1.0], 0) - 2.0).abs() < 1e-6);
        assert!(log_loss(&[0.5, 0.5], 0) > 0.69);
        assert!(ece(&[(0.9, true), (0.9, true)], 10) - 0.1 < 1e-5);
    }
}
