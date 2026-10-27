//! Old binary Office files (#70): Excel `.xls` (BIFF8), PowerPoint `.ppt`
//! and Word `.doc`, read on this computer without Office.
//!
//! All three are OLE compound files (a small FAT file system in one
//! file). [`Cfb`] reads its streams; then:
//!
//! - **.xls**: the `Workbook` stream's records: sheet names
//!   (BOUNDSHEET), the shared strings (SST, across CONTINUE records),
//!   and the cells (LABELSST, LABEL, NUMBER, RK, MULRK, numeric FORMULA
//!   results and their STRING records). One table per sheet.
//! - **.ppt** (best effort): the text atoms (TextCharsAtom,
//!   TextBytesAtom) of the `PowerPoint Document` stream, one section per
//!   slide (SlideListWithText order; master slides left out).
//! - **.doc** (best effort): the main document's text through the piece
//!   table (Clx) in the `0Table`/`1Table` stream.
//!
//! Formatting, pictures, charts and embedded objects are not read.

/// The most rows read from one sheet, and columns from one row.
const MAX_ROWS: usize = 300;
const MAX_COLS: usize = 40;
/// The most text kept from one file.
const MAX_CHARS: usize = 400_000;

const END: u32 = 0xFFFF_FFFE;
const FREE: u32 = 0xFFFF_FFFF;

/// An OLE compound file opened in memory.
pub struct Cfb<'a> {
    data: &'a [u8],
    sector: usize,
    fat: Vec<u32>,
    minifat: Vec<u32>,
    ministream: Vec<u8>,
    cutoff: u64,
    entries: Vec<(String, u8, u32, u64)>,
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// Whether bytes start like an OLE compound file.
pub fn is_cfb(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])
}

impl<'a> Cfb<'a> {
    pub fn open(data: &'a [u8]) -> Result<Self, String> {
        if !is_cfb(data) || data.len() < 512 {
            return Err("it is not an Office 97–2003 file".into());
        }
        let bad = || "the file looks damaged".to_owned();
        let shift = u16_at(data, 0x1E).ok_or_else(bad)?;
        if !(7..=16).contains(&shift) {
            return Err(bad());
        }
        let sector = 1usize << shift;
        let first_dir = u32_at(data, 0x30).ok_or_else(bad)?;
        let cutoff = u64::from(u32_at(data, 0x38).ok_or_else(bad)?);
        let first_minifat = u32_at(data, 0x3C).ok_or_else(bad)?;
        let mut first_difat = u32_at(data, 0x44).ok_or_else(bad)?;
        let n_difat = u32_at(data, 0x48).ok_or_else(bad)?;
        let mut cfb = Cfb {
            data,
            sector,
            fat: Vec::new(),
            minifat: Vec::new(),
            ministream: Vec::new(),
            cutoff,
            entries: Vec::new(),
        };
        // The FAT's sectors: 109 in the header, then the DIFAT chain.
        let mut fat_sectors: Vec<u32> = (0..109)
            .filter_map(|k| u32_at(data, 0x4C + k * 4))
            .filter(|&s| s < END - 4)
            .collect();
        let per = sector / 4;
        for _ in 0..n_difat.min(10_000) {
            if first_difat >= END - 4 {
                break;
            }
            let Some(sec) = cfb.sector_bytes(first_difat) else {
                break;
            };
            for k in 0..per - 1 {
                if let Some(s) = u32_at(sec, k * 4).filter(|&s| s < END - 4) {
                    fat_sectors.push(s);
                }
            }
            first_difat = u32_at(sec, (per - 1) * 4).unwrap_or(END);
        }
        for s in fat_sectors {
            let sec = cfb.sector_bytes(s).ok_or_else(bad)?;
            cfb.fat.extend((0..per).filter_map(|k| u32_at(sec, k * 4)));
        }
        // The directory.
        let dir = cfb.chain(first_dir);
        for e in dir.chunks_exact(128) {
            let len = usize::from(u16_at(e, 0x40).unwrap_or(0)).min(64);
            let name: Vec<u16> = (0..len.saturating_sub(2) / 2)
                .filter_map(|k| u16_at(e, k * 2))
                .collect();
            let name = String::from_utf16_lossy(&name);
            let kind = e[0x42];
            let start = u32_at(e, 0x74).unwrap_or(END);
            let size = u64::from(u32_at(e, 0x78).unwrap_or(0));
            cfb.entries.push((name, kind, start, size));
        }
        cfb.minifat = {
            let raw = cfb.chain(first_minifat);
            (0..raw.len() / 4)
                .filter_map(|k| u32_at(&raw, k * 4))
                .collect()
        };
        if let Some(&(_, 5, start, size)) = cfb.entries.first() {
            let mut ms = cfb.chain(start);
            ms.truncate(usize::try_from(size).unwrap_or(usize::MAX));
            cfb.ministream = ms;
        }
        Ok(cfb)
    }

    fn sector_bytes(&self, n: u32) -> Option<&'a [u8]> {
        let start = (n as usize).checked_add(1)?.checked_mul(self.sector)?;
        self.data.get(start..start + self.sector)
    }

    /// A FAT chain's bytes, from `start`.
    fn chain(&self, mut s: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut steps = 0;
        while s < END - 4 && steps <= self.fat.len() {
            let Some(b) = self.sector_bytes(s) else { break };
            out.extend_from_slice(b);
            s = self.fat.get(s as usize).copied().unwrap_or(END);
            steps += 1;
        }
        out
    }

    fn mini_chain(&self, mut s: u32, size: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut steps = 0;
        while s < END - 4 && steps <= self.minifat.len() && out.len() < size {
            let at = s as usize * 64;
            let Some(b) = self.ministream.get(at..at + 64) else {
                break;
            };
            out.extend_from_slice(b);
            s = self.minifat.get(s as usize).copied().unwrap_or(END);
            steps += 1;
        }
        out.truncate(size);
        out
    }

    /// The stream named `name` (case-insensitive), if there is one.
    pub fn stream(&self, name: &str) -> Option<Vec<u8>> {
        let &(_, _, start, size) = self
            .entries
            .iter()
            .find(|(n, kind, _, _)| *kind == 2 && n.eq_ignore_ascii_case(name))?;
        let size = usize::try_from(size).ok()?;
        if (size as u64) < self.cutoff {
            Some(self.mini_chain(start, size))
        } else {
            let mut v = self.chain(start);
            v.truncate(size);
            Some(v)
        }
    }

    /// The names of the streams.
    pub fn names(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.1 == 2)
            .map(|e| e.0.clone())
            .collect()
    }
}

// ----- Excel (BIFF8) -----

/// The records of a BIFF stream: (type, data).
fn records(s: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= s.len() {
        let (Some(t), Some(n)) = (u16_at(s, i), u16_at(s, i + 2)) else {
            break;
        };
        let n = usize::from(n);
        let Some(d) = s.get(i + 4..i + 4 + n) else {
            break;
        };
        out.push((t, d));
        i += 4 + n;
    }
    out
}

/// Reads strings spread over a record and its CONTINUE records, where a
/// string's characters may go on in the next record with a new flag byte.
struct Pieces<'a> {
    parts: Vec<&'a [u8]>,
    part: usize,
    pos: usize,
}

impl Pieces<'_> {
    fn byte(&mut self) -> Option<u8> {
        loop {
            let p = self.parts.get(self.part)?;
            if let Some(&b) = p.get(self.pos) {
                self.pos += 1;
                return Some(b);
            }
            self.part += 1;
            self.pos = 0;
        }
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes([self.byte()?, self.byte()?]))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes([
            self.byte()?,
            self.byte()?,
            self.byte()?,
            self.byte()?,
        ]))
    }

    fn skip(&mut self, n: usize) {
        for _ in 0..n {
            if self.byte().is_none() {
                return;
            }
        }
    }

    /// An XLUnicodeRichExtendedString.
    fn string(&mut self) -> Option<String> {
        let cch = usize::from(self.u16()?);
        let flags = self.byte()?;
        let runs = if flags & 0x08 != 0 { self.u16()? } else { 0 };
        let ext = if flags & 0x04 != 0 { self.u32()? } else { 0 };
        let mut wide = flags & 0x01 != 0;
        let mut out = String::new();
        let mut units: Vec<u16> = Vec::new();
        let mut left = cch;
        while left > 0 {
            // At the end of a record, the rest starts with a flag byte.
            if self
                .parts
                .get(self.part)
                .is_some_and(|p| self.pos >= p.len())
            {
                self.part += 1;
                self.pos = 0;
                self.parts.get(self.part)?;
                wide = self.byte()? & 0x01 != 0;
            }
            if wide {
                units.push(self.u16()?);
            } else {
                let b = self.byte()?;
                units.push(u16::from(b));
            }
            left -= 1;
        }
        out.push_str(&String::from_utf16_lossy(&units));
        self.skip(usize::from(runs) * 4 + ext as usize);
        Some(out)
    }
}

/// A short or long XLUnicodeString inside one record: `len_bytes` 1 or 2.
fn record_string(d: &[u8], at: usize, len_bytes: usize) -> String {
    let cch = if len_bytes == 1 {
        d.get(at).map_or(0, |&b| usize::from(b))
    } else {
        u16_at(d, at).map_or(0, usize::from)
    };
    let flags = d.get(at + len_bytes).copied().unwrap_or(0);
    let s = at + len_bytes + 1;
    if flags & 0x01 != 0 {
        let units: Vec<u16> = (0..cch).filter_map(|k| u16_at(d, s + k * 2)).collect();
        String::from_utf16_lossy(&units)
    } else {
        d.get(s..(s + cch).min(d.len()))
            .unwrap_or_default()
            .iter()
            .map(|&b| char::from(b))
            .collect()
    }
}

/// An RK number.
fn rk(v: u32) -> f64 {
    let n = if v & 0x02 != 0 {
        f64::from((v as i32) >> 2)
    } else {
        f64::from_bits(u64::from(v & 0xFFFF_FFFC) << 32)
    };
    if v & 0x01 != 0 { n / 100.0 } else { n }
}

fn number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// An Excel 97–2003 workbook: (sheet name, rows of cells).
pub fn xls_sheets(bytes: &[u8]) -> Result<Vec<(String, Vec<Vec<String>>)>, String> {
    let cfb = Cfb::open(bytes)?;
    let book = cfb
        .stream("Workbook")
        .or_else(|| cfb.stream("Book"))
        .ok_or("it has no Workbook stream (an Excel 5 or older file?)")?;
    let recs = records(&book);
    let mut names = Vec::new();
    let mut sst: Vec<String> = Vec::new();
    for (k, (t, d)) in recs.iter().enumerate() {
        match t {
            0x0085 if d.get(5) == Some(&0) => names.push(record_string(d, 6, 1)),
            0x00FC => {
                let mut parts = vec![d.get(8..).unwrap_or_default()];
                parts.extend(
                    recs[k + 1..]
                        .iter()
                        .take_while(|(t, _)| *t == 0x003C)
                        .map(|(_, d)| *d),
                );
                let unique = u32_at(d, 4).unwrap_or(0) as usize;
                let mut p = Pieces {
                    parts,
                    part: 0,
                    pos: 0,
                };
                for _ in 0..unique.min(1_000_000) {
                    match p.string() {
                        Some(s) => sst.push(s),
                        None => break,
                    }
                }
            }
            _ => {}
        }
    }
    let mut sheets: Vec<Vec<Vec<String>>> = Vec::new();
    let mut current: Option<usize> = None;
    let mut formula_at: Option<(usize, usize)> = None;
    fn put(sheets: &mut [Vec<Vec<String>>], cur: Option<usize>, r: usize, c: usize, v: String) {
        let Some(s) = cur.and_then(|i| sheets.get_mut(i)) else {
            return;
        };
        if r >= MAX_ROWS || c >= MAX_COLS {
            return;
        }
        if s.len() <= r {
            s.resize(r + 1, Vec::new());
        }
        if s[r].len() <= c {
            s[r].resize(c + 1, String::new());
        }
        s[r][c] = v;
    }
    for (t, d) in &recs {
        let rc = || {
            (
                u16_at(d, 0).map_or(usize::MAX, usize::from),
                u16_at(d, 2).map_or(usize::MAX, usize::from),
            )
        };
        match t {
            0x0809 => {
                // BOF of a worksheet.
                if u16_at(d, 2) == Some(0x0010) {
                    sheets.push(Vec::new());
                    current = Some(sheets.len() - 1);
                } else {
                    current = None;
                }
            }
            0x000A => current = None,
            0x00FD => {
                let (r, c) = rc();
                let i = u32_at(d, 6).unwrap_or(u32::MAX) as usize;
                put(
                    &mut sheets,
                    current,
                    r,
                    c,
                    sst.get(i).cloned().unwrap_or_default(),
                );
            }
            0x0204 => {
                let (r, c) = rc();
                put(&mut sheets, current, r, c, record_string(d, 6, 2));
            }
            0x0203 => {
                let (r, c) = rc();
                if let Some(b) = d.get(6..14) {
                    let n = f64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
                    put(&mut sheets, current, r, c, number(n));
                }
            }
            0x027E => {
                let (r, c) = rc();
                put(
                    &mut sheets,
                    current,
                    r,
                    c,
                    number(rk(u32_at(d, 6).unwrap_or(0))),
                );
            }
            0x00BD => {
                let (r, c0) = rc();
                let n = d.len().saturating_sub(6) / 6;
                for k in 0..n {
                    let v = rk(u32_at(d, 4 + k * 6 + 2).unwrap_or(0));
                    put(&mut sheets, current, r, c0.saturating_add(k), number(v));
                }
            }
            0x0006 => {
                let (r, c) = rc();
                formula_at = None;
                if let Some(b) = d.get(6..14) {
                    if b[6] == 0xFF && b[7] == 0xFF {
                        // A string result: in the STRING record next.
                        if b[0] == 0 {
                            formula_at = Some((r, c));
                        }
                    } else {
                        let n = f64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
                        put(&mut sheets, current, r, c, number(n));
                    }
                }
            }
            0x0207 => {
                if let Some((r, c)) = formula_at.take() {
                    put(&mut sheets, current, r, c, record_string(d, 0, 2));
                }
            }
            _ => {}
        }
    }
    if sheets.is_empty() {
        return Err("it has no sheets".into());
    }
    Ok(sheets
        .into_iter()
        .enumerate()
        .map(|(i, rows)| {
            let name = names
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("Sheet {}", i + 1));
            let rows = rows
                .into_iter()
                .filter(|r| r.iter().any(|c| !c.trim().is_empty()))
                .collect();
            (name, rows)
        })
        .collect())
}

// ----- PowerPoint -----

/// A PowerPoint 97–2003 file's text, slide by slide.
pub fn ppt_slides(bytes: &[u8]) -> Result<Vec<Vec<String>>, String> {
    let cfb = Cfb::open(bytes)?;
    let doc = cfb
        .stream("PowerPoint Document")
        .ok_or("it has no PowerPoint Document stream")?;
    let mut st = PptWalk::default();
    st.walk(&doc, 0, 0);
    let slides = if st.listed.iter().any(|s| !s.is_empty()) {
        st.listed
    } else if st.drawn.iter().any(|s| !s.is_empty()) {
        st.drawn
    } else if !st.loose.is_empty() {
        vec![st.loose]
    } else {
        return Err("Tern found no text in it".into());
    };
    Ok(slides.into_iter().filter(|s| !s.is_empty()).collect())
}

#[derive(Default)]
struct PptWalk {
    /// SlideListWithText: one entry per SlidePersistAtom.
    listed: Vec<Vec<String>>,
    /// Text inside Slide containers, one entry per slide.
    drawn: Vec<Vec<String>>,
    loose: Vec<String>,
    chars: usize,
}

impl PptWalk {
    /// `mode` 1: inside the slides' SlideListWithText; 2: in a Slide;
    /// 3: in another SlideListWithText (notes, masters), not read.
    fn walk(&mut self, b: &[u8], depth: usize, mode: u8) {
        if depth > 12 {
            return;
        }
        let mut i = 0;
        while i + 8 <= b.len() && self.chars < MAX_CHARS {
            let ver_inst = u16_at(b, i).unwrap_or(0);
            let t = u16_at(b, i + 2).unwrap_or(0);
            let len = u32_at(b, i + 4).unwrap_or(0) as usize;
            let body = &b[i + 8..(i + 8).saturating_add(len).min(b.len())];
            let container = ver_inst & 0x000F == 0x000F;
            let instance = ver_inst >> 4;
            match t {
                // MainMaster, Notes masters, handout: no slide text.
                0x03F8 | 0x0FC9 => {}
                0x0FF0 if container => {
                    let m = if instance == 0 { 1 } else { 3 };
                    self.walk(body, depth + 1, m);
                }
                0x03EE if container => {
                    self.drawn.push(Vec::new());
                    self.walk(body, depth + 1, 2);
                }
                0x03F3 if mode == 1 => self.listed.push(Vec::new()),
                0x0FA0 | 0x0FA8 => {
                    let text = if t == 0x0FA0 {
                        let u: Vec<u16> = (0..body.len() / 2)
                            .filter_map(|k| u16_at(body, k * 2))
                            .collect();
                        String::from_utf16_lossy(&u)
                    } else {
                        body.iter().map(|&c| char::from(c)).collect()
                    };
                    let lines: Vec<String> = text
                        .split(['\r', '\n', '\u{b}'])
                        .map(|l| l.trim().to_owned())
                        .filter(|l| !l.is_empty() && !l.starts_with("Click to edit"))
                        .collect();
                    self.chars += lines.iter().map(String::len).sum::<usize>();
                    let target = match mode {
                        1 => self.listed.last_mut(),
                        2 => self.drawn.last_mut(),
                        _ => None,
                    };
                    match target {
                        Some(v) => v.extend(lines),
                        None if mode != 3 => self.loose.extend(lines),
                        None => {}
                    }
                }
                _ if container => self.walk(body, depth + 1, mode),
                _ => {}
            }
            i += 8 + len;
        }
    }
}

// ----- Word -----

/// A Word 97–2003 document's main text.
pub fn doc_text(bytes: &[u8]) -> Result<String, String> {
    let cfb = Cfb::open(bytes)?;
    let wd = cfb
        .stream("WordDocument")
        .ok_or("it has no WordDocument stream")?;
    if u16_at(&wd, 0) != Some(0xA5EC) {
        return Err("it is not a Word 97–2003 document".into());
    }
    let flags = u16_at(&wd, 0x0A).unwrap_or(0);
    if flags & 0x0100 != 0 {
        return Err("the document is password-protected".into());
    }
    let table = cfb
        .stream(if flags & 0x0200 != 0 {
            "1Table"
        } else {
            "0Table"
        })
        .ok_or("it has no table stream")?;
    let ccp_text = u32_at(&wd, 0x4C).unwrap_or(0) as usize;
    let fc_clx = u32_at(&wd, 0x01A2).unwrap_or(0) as usize;
    let lcb_clx = u32_at(&wd, 0x01A6).unwrap_or(0) as usize;
    let clx = table
        .get(fc_clx..fc_clx.saturating_add(lcb_clx).min(table.len()))
        .ok_or("the piece table is missing")?;
    // Skip the property modifiers (Prc), find the piece table (Pcdt).
    let mut i = 0;
    while clx.get(i) == Some(&0x01) {
        i += 3 + u16_at(clx, i + 1).map_or(0, usize::from);
    }
    if clx.get(i) != Some(&0x02) {
        return Err("the piece table is damaged".into());
    }
    let lcb = u32_at(clx, i + 1).unwrap_or(0) as usize;
    let plc = clx
        .get(i + 5..(i + 5 + lcb).min(clx.len()))
        .unwrap_or_default();
    let n = plc.len().saturating_sub(4) / 12;
    let mut out = String::new();
    let mut taken = 0usize;
    for k in 0..n {
        let (Some(cp0), Some(cp1)) = (u32_at(plc, k * 4), u32_at(plc, k * 4 + 4)) else {
            break;
        };
        let pcd = (n + 1) * 4 + k * 8;
        let fc = u32_at(plc, pcd + 2).unwrap_or(0);
        let count = (cp1.saturating_sub(cp0) as usize).min(ccp_text.saturating_sub(taken));
        if count == 0 {
            continue;
        }
        if fc & 0x4000_0000 != 0 {
            let at = ((fc & !0x4000_0000) / 2) as usize;
            if let Some(b) = wd.get(at..(at + count).min(wd.len())) {
                out.extend(b.iter().map(|&c| cp1252(c)));
            }
        } else {
            let at = fc as usize;
            let u: Vec<u16> = (0..count).filter_map(|j| u16_at(&wd, at + j * 2)).collect();
            out.push_str(&String::from_utf16_lossy(&u));
        }
        taken += count;
        if taken >= ccp_text || out.len() > MAX_CHARS {
            break;
        }
    }
    Ok(clean_doc(&out))
}

/// The few Windows-1252 letters that differ from Latin-1.
fn cp1252(b: u8) -> char {
    match b {
        0x91 | 0x92 => '\'',
        0x93 | 0x94 => '"',
        0x96 | 0x97 => '-',
        0x85 => '…',
        0x80 => '€',
        _ => char::from(b),
    }
}

/// Word's control characters to plain text: paragraph marks to new
/// lines, cell marks to tabs, field instructions dropped.
fn clean_doc(s: &str) -> String {
    let mut out = String::new();
    let mut field = 0usize;
    for c in s.chars() {
        match c {
            '\u{13}' => field += 1,
            '\u{14}' => field = field.saturating_sub(1),
            '\u{15}' => {}
            _ if field > 0 => {}
            '\r' | '\u{b}' | '\u{c}' => out.push('\n'),
            '\u{7}' => out.push('\t'),
            '\t' | '\n' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    let lines: Vec<&str> = out.lines().map(str::trim_end).collect();
    let mut tidy = lines.join("\n");
    while tidy.contains("\n\n\n") {
        tidy = tidy.replace("\n\n\n", "\n\n");
    }
    tidy.trim().to_owned()
}

/// A compound file holding `streams`, each padded to at least 4096 bytes
/// (no mini stream); 512-byte sectors, up to 3 streams (tests, the tour).
pub fn sample_cfb(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
    const S: usize = 512;
    let mut header = vec![0u8; S];
    header[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    header[0x18..0x1A].copy_from_slice(&0x3Eu16.to_le_bytes());
    header[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
    header[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
    header[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
    header[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
    header[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes());
    header[0x30..0x34].copy_from_slice(&1u32.to_le_bytes());
    header[0x38..0x3C].copy_from_slice(&4096u32.to_le_bytes());
    header[0x3C..0x40].copy_from_slice(&END.to_le_bytes());
    header[0x44..0x48].copy_from_slice(&END.to_le_bytes());
    for k in 0..109 {
        let v = if k == 0 { 0 } else { FREE };
        header[0x4C + k * 4..0x50 + k * 4].copy_from_slice(&v.to_le_bytes());
    }
    let mut fat = vec![FREE; S / 4];
    fat[0] = 0xFFFF_FFFD;
    fat[1] = END;
    let mut dir = vec![0u8; S];
    let mut entry = |k: usize, name: &str, kind: u8, start: u32, size: u32| {
        let e = &mut dir[k * 128..(k + 1) * 128];
        let units: Vec<u16> = name.encode_utf16().collect();
        for (j, u) in units.iter().enumerate().take(31) {
            e[j * 2..j * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        let len = ((units.len().min(31) + 1) * 2) as u16;
        e[0x40..0x42].copy_from_slice(&len.to_le_bytes());
        e[0x42] = kind;
        e[0x44..0x48].copy_from_slice(&FREE.to_le_bytes());
        e[0x48..0x4C].copy_from_slice(&FREE.to_le_bytes());
        e[0x4C..0x50].copy_from_slice(&FREE.to_le_bytes());
        e[0x74..0x78].copy_from_slice(&start.to_le_bytes());
        e[0x78..0x7C].copy_from_slice(&size.to_le_bytes());
    };
    entry(0, "Root Entry", 5, END, 0);
    let mut data = Vec::new();
    let mut next = 2usize;
    for (k, (name, bytes)) in streams.iter().take(3).enumerate() {
        let mut b = bytes.clone();
        if b.len() < 4096 {
            b.resize(4096, 0);
        }
        let n = b.len().div_ceil(S);
        entry(k + 1, name, 2, next as u32, b.len() as u32);
        for j in 0..n {
            fat[next + j] = if j + 1 == n {
                END
            } else {
                (next + j + 1) as u32
            };
        }
        b.resize(n * S, 0);
        data.extend(b);
        next += n;
    }
    let mut out = header;
    out.extend(fat.iter().flat_map(|v| v.to_le_bytes()));
    out.extend(dir);
    out.extend(data);
    out
}

/// A BIFF record.
pub fn biff(t: u16, d: &[u8]) -> Vec<u8> {
    let mut v = t.to_le_bytes().to_vec();
    v.extend((d.len() as u16).to_le_bytes());
    v.extend_from_slice(d);
    v
}

/// A small `.xls`: sheet "Costs" with Item/Total, Tea/4250 (tests, tour).
pub fn sample_xls() -> Vec<u8> {
    let mut w = Vec::new();
    w.extend(biff(0x0809, &[0x00, 0x06, 0x05, 0x00, 0, 0, 0, 0]));
    let mut bs = vec![0, 0, 0, 0, 0, 0, 5, 0];
    bs.extend_from_slice(b"Costs");
    w.extend(biff(0x0085, &bs));
    let mut sst = Vec::new();
    sst.extend(3u32.to_le_bytes());
    sst.extend(3u32.to_le_bytes());
    for s in ["Item", "Total", "Tea"] {
        sst.extend((s.len() as u16).to_le_bytes());
        sst.push(0);
        sst.extend_from_slice(s.as_bytes());
    }
    w.extend(biff(0x00FC, &sst));
    w.extend(biff(0x000A, &[]));
    w.extend(biff(0x0809, &[0x00, 0x06, 0x10, 0x00, 0, 0, 0, 0]));
    let label = |r: u16, c: u16, i: u32| {
        let mut d = r.to_le_bytes().to_vec();
        d.extend(c.to_le_bytes());
        d.extend(0u16.to_le_bytes());
        d.extend(i.to_le_bytes());
        biff(0x00FD, &d)
    };
    w.extend(label(0, 0, 0));
    w.extend(label(0, 1, 1));
    w.extend(label(1, 0, 2));
    let mut n = 1u16.to_le_bytes().to_vec();
    n.extend(1u16.to_le_bytes());
    n.extend(0u16.to_le_bytes());
    n.extend(4250f64.to_le_bytes());
    w.extend(biff(0x0203, &n));
    w.extend(biff(0x000A, &[]));
    sample_cfb(&[("Workbook", w)])
}

/// A small `.ppt`: two slides through SlideListWithText (tests, tour).
pub fn sample_ppt() -> Vec<u8> {
    let atom = |ver_inst: u16, t: u16, body: &[u8]| {
        let mut v = ver_inst.to_le_bytes().to_vec();
        v.extend(t.to_le_bytes());
        v.extend((body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        v
    };
    let chars = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(u16::to_le_bytes).collect() };
    let mut list = Vec::new();
    list.extend(atom(0, 0x03F3, &[0; 20]));
    list.extend(atom(0, 0x0FA0, &chars("Intro\rHello from Lahore")));
    list.extend(atom(0, 0x03F3, &[0; 20]));
    list.extend(atom(0, 0x0FA8, b"Plan\rShip #70"));
    let slwt = atom(0x000F, 0x0FF0, &list);
    let docc = atom(0x000F, 0x03E8, &slwt);
    sample_cfb(&[("PowerPoint Document", docc)])
}

/// A small `.doc` (one piece, 8-bit text) (tests, tour).
pub fn sample_doc(text: &str) -> Vec<u8> {
    let mut wd = vec![0u8; 2048];
    wd[0..2].copy_from_slice(&0xA5ECu16.to_le_bytes());
    wd[0x0A..0x0C].copy_from_slice(&0x0200u16.to_le_bytes());
    let body: Vec<u8> = text.bytes().collect();
    wd[0x4C..0x50].copy_from_slice(&(body.len() as u32).to_le_bytes());
    let at = 1024usize;
    wd[at..at + body.len()].copy_from_slice(&body);
    // Clx: a Pcdt with one piece.
    let mut plc = Vec::new();
    plc.extend(0u32.to_le_bytes());
    plc.extend((body.len() as u32).to_le_bytes());
    plc.extend(0u16.to_le_bytes());
    plc.extend((((at * 2) as u32) | 0x4000_0000).to_le_bytes());
    plc.extend(0u16.to_le_bytes());
    let mut clx = vec![0x02];
    clx.extend((plc.len() as u32).to_le_bytes());
    clx.extend(plc);
    let mut table = vec![0u8; 64];
    table.extend(&clx);
    wd[0x01A2..0x01A6].copy_from_slice(&64u32.to_le_bytes());
    wd[0x01A6..0x01AA].copy_from_slice(&(clx.len() as u32).to_le_bytes());
    sample_cfb(&[("WordDocument", wd), ("1Table", table)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_old_excel_workbook() {
        let sheets = xls_sheets(&sample_xls()).unwrap();
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].0, "Costs");
        assert_eq!(
            sheets[0].1,
            vec![
                vec!["Item".to_owned(), "Total".to_owned()],
                vec!["Tea".to_owned(), "4250".to_owned()]
            ]
        );
    }

    #[test]
    fn reads_old_powerpoint_slides() {
        let s = ppt_slides(&sample_ppt()).unwrap();
        assert_eq!(
            s,
            vec![
                vec!["Intro".to_owned(), "Hello from Lahore".to_owned()],
                vec!["Plan".to_owned(), "Ship #70".to_owned()]
            ]
        );
    }

    #[test]
    fn reads_an_old_word_document() {
        let t = doc_text(&sample_doc("Meeting notes\rThe demo is on Friday.\r")).unwrap();
        assert_eq!(t, "Meeting notes\nThe demo is on Friday.");
    }

    #[test]
    fn rk_numbers_and_strings_across_continue_records() {
        assert_eq!(rk((42 << 2) | 0x02), 42.0);
        assert_eq!(rk((4250 << 2) | 0x03), 42.5);
        // "Hello" starts 8-bit in one record and goes on 16-bit in the next.
        let a: &[u8] = &[5, 0, 0, b'H', b'e'];
        let b: &[u8] = &[1, b'l', 0, b'l', 0, b'o', 0];
        let mut p = Pieces {
            parts: vec![a, b],
            part: 0,
            pos: 0,
        };
        assert_eq!(p.string().as_deref(), Some("Hello"));
    }

    #[test]
    fn junk_is_an_error_not_a_panic() {
        assert!(xls_sheets(b"not an office file at all").is_err());
        let mut x = sample_xls();
        x.truncate(700);
        let _ = xls_sheets(&x);
        let _ = ppt_slides(&x);
        let _ = doc_text(&x);
    }
}
