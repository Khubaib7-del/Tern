//! Reading PaddleOCR's exported `inference.yml` files: the recogniser's
//! character dictionary and the detector's post-processing thresholds.
//! GPUI- and runtime-free, so it is tested in every build; the ONNX engine
//! that uses it is [`super::onnx_ocr`].

/// The characters in a recogniser's `PostProcess.character_dict`, one per
/// entry, in order (the model's class index minus one). PaddleOCR writes
/// the list as YAML block items, plain or quoted.
pub fn character_dict(yml: &str) -> Result<Vec<String>, String> {
    let mut lines = yml.lines();
    let indent = loop {
        let Some(l) = lines.next() else {
            return Err("no character_dict in inference.yml".into());
        };
        if l.trim_start() == "character_dict:" {
            break l.len() - l.trim_start().len();
        }
    };
    let mut out = Vec::new();
    for l in lines {
        let lead = l.len() - l.trim_start().len();
        let t = l.trim_start();
        if let Some(item) = t.strip_prefix("- ").or_else(|| (t == "-").then_some(""))
            && lead >= indent
        {
            out.push(scalar(item)?);
        } else if t.is_empty() {
            continue;
        } else {
            break;
        }
    }
    if out.is_empty() {
        return Err("the character_dict in inference.yml is empty".into());
    }
    Ok(out)
}

/// One YAML scalar as PaddleOCR writes them: `'x'` (with `''` for a
/// quote), `"x"` (with backslash escapes), or plain.
fn scalar(s: &str) -> Result<String, String> {
    let s = s.trim_end();
    if let Some(inner) = s.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        return Ok(inner.replace("''", "'"));
    }
    if let Some(inner) = s.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        let mut out = String::new();
        let mut it = inner.chars();
        while let Some(c) = it.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            let hex = |it: &mut std::str::Chars, n: usize| -> Result<char, String> {
                let h: String = it.by_ref().take(n).collect();
                u32::from_str_radix(&h, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("bad escape \\{h} in {s}"))
            };
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some(' ') => out.push(' '),
                Some('_') => out.push('\u{a0}'),
                Some('N') => out.push('\u{85}'),
                Some('L') => out.push('\u{2028}'),
                Some('P') => out.push('\u{2029}'),
                Some('x') => out.push(hex(&mut it, 2)?),
                Some('u') => out.push(hex(&mut it, 4)?),
                Some('U') => out.push(hex(&mut it, 8)?),
                Some(c) => out.push(c),
                None => return Err(format!("dangling escape in {s}")),
            }
        }
        return Ok(out);
    }
    Ok(s.to_owned())
}

/// The detector's `DBPostProcess` thresholds, when `inference.yml` names
/// them: `(thresh, box_thresh, unclip_ratio, max_candidates)`.
pub fn det_thresholds(yml: &str) -> (Option<f32>, Option<f32>, Option<f32>, Option<usize>) {
    let mut in_post = false;
    let (mut t, mut b, mut u, mut m) = (None, None, None, None);
    for l in yml.lines() {
        if !l.starts_with(' ') {
            in_post = l.trim_end() == "PostProcess:";
            continue;
        }
        if !in_post {
            continue;
        }
        let Some((k, v)) = l.trim().split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k {
            "thresh" => t = v.parse().ok(),
            "box_thresh" => b = v.parse().ok(),
            "unclip_ratio" => u = v.parse().ok(),
            "max_candidates" => m = v.parse().ok(),
            _ => {}
        }
    }
    (t, b, u, m)
}

/// Merges two readings of the same detected lines (same detector, same
/// picture, so the same regions in the same order): each line keeps the
/// reading the recognisers were surer of.
pub fn pick_lines(a: &[(String, f32)], b: &[(String, f32)]) -> Vec<String> {
    if a.len() != b.len() {
        let mean =
            |v: &[(String, f32)]| v.iter().map(|(_, c)| *c).sum::<f32>() / v.len().max(1) as f32;
        let best = if mean(b) > mean(a) { b } else { a };
        return best.iter().map(|(t, _)| t.clone()).collect();
    }
    a.iter()
        .zip(b)
        .map(|(x, y)| if y.1 > x.1 { y.0.clone() } else { x.0.clone() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REC: &str = "Global:\n  model_name: x\nPostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - '!'\n  - '\"'\n  - ''''\n  - a\n  - \"\\u3000\"\n  - ب\n  - 🛅\n";

    #[test]
    fn reads_the_dictionary_with_quotes_and_escapes() {
        assert_eq!(
            character_dict(REC).unwrap(),
            ["!", "\"", "'", "a", "\u{3000}", "ب", "🛅"]
        );
        assert!(character_dict("Global:\n  x: 1\n").is_err());
    }

    #[test]
    fn the_dictionary_ends_at_the_next_key() {
        let y = "PostProcess:\n  character_dict:\n  - a\n  - b\n  name: CTCLabelDecode\nPreProcess:\n  - c\n";
        assert_eq!(character_dict(y).unwrap(), ["a", "b"]);
    }

    #[test]
    fn reads_the_detector_thresholds() {
        let y = "Global:\n  model_name: d\nPostProcess:\n  box_thresh: 0.45\n  max_candidates: 3000\n  name: DBPostProcess\n  thresh: 0.2\n  unclip_ratio: 1.4\nPreProcess:\n  thresh: 9\n";
        assert_eq!(
            det_thresholds(y),
            (Some(0.2), Some(0.45), Some(1.4), Some(3000))
        );
    }

    #[test]
    fn each_line_keeps_the_surer_reading() {
        let latin = [("Hello".to_owned(), 0.98), ("x;l".to_owned(), 0.41)];
        let arabic = [("ﻩ".to_owned(), 0.30), ("سلام".to_owned(), 0.93)];
        assert_eq!(pick_lines(&latin, &arabic), ["Hello", "سلام"]);
        assert_eq!(pick_lines(&latin, &arabic[..1]), ["Hello", "x;l"]);
    }
}
