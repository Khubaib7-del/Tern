//! Anki package export: a real `.apkg`.
//!
//! An `.apkg` is a zip holding `collection.anki2` (an SQLite database in
//! Anki's schema 11, which every current Anki still imports) and `media`
//! (a JSON map of media files; empty here). The collection carries two
//! note types, "Tern Basic" (Front, Back, Source) and "Tern Cloze" (Text,
//! Back Extra, Source), one deck, the notes with their tags, and one card
//! per note (one per cloze number for cloze notes). Cards in review keep
//! their SM-2 interval, ease, and due day, so the schedule carries over;
//! cards still learning go in as new. The review log is not exported.
//!
//! The writer takes plain data, so it knows nothing of Tern's study types.

use std::io::{Read, Write};
use std::path::Path;

use rusqlite::{Connection, params};
use serde_json::json;

const DAY: i64 = 86_400;
/// Fixed ids, so re-importing updates the same note types.
pub const BASIC_MODEL_ID: i64 = 1_729_000_000_001;
pub const CLOZE_MODEL_ID: i64 = 1_729_000_000_002;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    /// Fields: Front, Back, Source.
    Basic,
    /// Fields: Text, Back Extra, Source.
    Cloze,
}

/// A card's schedule as Anki stores it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReviewState {
    /// Interval in days.
    pub interval: i64,
    /// Ease as per mille (2500 is 250%).
    pub factor: i64,
    pub reps: i64,
    pub lapses: i64,
    /// When the card is due, Unix seconds.
    pub due: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub kind: NoteKind,
    /// Three fields, HTML.
    pub fields: [String; 3],
    /// Tags, no spaces in any.
    pub tags: Vec<String>,
    /// Stable text the note's GUID is made from (Tern's card id), so
    /// importing again updates instead of duplicating.
    pub guid_seed: String,
    /// Cloze numbers used (`[1, 2]`); ignored for basic notes.
    pub clozes: Vec<u32>,
    /// `None`: a new card.
    pub review: Option<ReviewState>,
    pub suspended: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    pub deck: String,
    pub notes: Vec<Note>,
    /// Unix seconds; ids and the collection's creation day come from it.
    pub created: i64,
}

/// What a written or read package holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub notes: usize,
    pub cards: usize,
    pub decks: Vec<String>,
    pub models: Vec<String>,
}

/// SHA-1 of `s` as hex.
fn sha1_hex(s: &str) -> String {
    sha1_smol::Sha1::from(s).digest().to_string()
}

/// Anki's field checksum: the first 8 hex digits of the SHA-1 of the
/// field with its HTML stripped, as a number.
pub fn checksum(field: &str) -> i64 {
    i64::from_str_radix(&sha1_hex(&strip_html(field))[..8], 16).unwrap_or(0)
}

/// Text without HTML tags, for the sort field and the checksum.
pub fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .trim()
        .to_owned()
}

/// A short GUID from a seed, in Anki's base-91 alphabet.
fn guid(seed: &str) -> String {
    const ABC: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!#$%&()*+,-./:;<=>?@[]^_`{|}~";
    let h = sha1_hex(seed);
    let mut n = u64::from_str_radix(&h[..16], 16).unwrap_or(1);
    let mut out = String::new();
    while n > 0 {
        out.push(ABC[(n % ABC.len() as u64) as usize] as char);
        n /= ABC.len() as u64;
    }
    out
}

/// A deck id from its name: stable and well clear of Default (1).
pub fn deck_id(name: &str) -> i64 {
    1_000_000_000 + i64::from_str_radix(&sha1_hex(name)[..7], 16).unwrap_or(0)
}

const SCHEMA: &str = "
CREATE TABLE col (id integer primary key, crt integer not null, mod integer not null, scm integer not null, ver integer not null, dty integer not null, usn integer not null, ls integer not null, conf text not null, models text not null, decks text not null, dconf text not null, tags text not null);
CREATE TABLE notes (id integer primary key, guid text not null, mid integer not null, mod integer not null, usn integer not null, tags text not null, flds text not null, sfld integer not null, csum integer not null, flags integer not null, data text not null);
CREATE TABLE cards (id integer primary key, nid integer not null, did integer not null, ord integer not null, mod integer not null, usn integer not null, type integer not null, queue integer not null, due integer not null, ivl integer not null, factor integer not null, reps integer not null, lapses integer not null, left integer not null, odue integer not null, odid integer not null, flags integer not null, data text not null);
CREATE TABLE revlog (id integer primary key, cid integer not null, usn integer not null, ease integer not null, ivl integer not null, lastIvl integer not null, factor integer not null, time integer not null, type integer not null);
CREATE TABLE graves (usn integer not null, oid integer not null, type integer not null);
CREATE INDEX ix_notes_usn on notes (usn);
CREATE INDEX ix_cards_usn on cards (usn);
CREATE INDEX ix_revlog_usn on revlog (usn);
CREATE INDEX ix_cards_nid on cards (nid);
CREATE INDEX ix_cards_sched on cards (did, queue, due);
CREATE INDEX ix_revlog_cid on revlog (cid);
CREATE INDEX ix_notes_csum on notes (csum);
";

const CSS: &str = ".card { font-family: \"IBM Plex Sans\", Arial, sans-serif; font-size: 20px; text-align: left; color: #141414; background-color: #FBFBFA; line-height: 1.45; }\n.cloze { font-weight: 600; color: #56603F; }\n.source { margin-top: 1em; font-size: 13px; color: #5A5A57; }\n.nightMode .card, .night_mode .card { color: #EDEDED; background-color: #131313; }\n.nightMode .cloze, .night_mode .cloze { color: #A3AE8E; }\n";

const LATEX_PRE: &str = "\\documentclass[12pt]{article}\n\\special{papersize=3in,5in}\n\\usepackage[utf8]{inputenc}\n\\usepackage{amssymb,amsmath}\n\\pagestyle{empty}\n\\setlength{\\parindent}{0in}\n\\begin{document}\n";

fn field_json(name: &str, ord: usize) -> serde_json::Value {
    json!({"name": name, "ord": ord, "sticky": false, "rtl": false, "font": "Arial", "size": 20, "media": []})
}

fn models_json(did: i64, now: i64) -> serde_json::Value {
    let basic = json!({
        "id": BASIC_MODEL_ID, "name": "Tern Basic", "type": 0, "mod": now, "usn": -1,
        "sortf": 0, "did": did,
        "tmpls": [{"name": "Card 1", "ord": 0,
            "qfmt": "{{Front}}",
            "afmt": "{{FrontSide}}\n\n<hr id=answer>\n\n{{Back}}\n{{#Source}}<div class=\"source\">{{Source}}</div>{{/Source}}",
            "bqfmt": "", "bafmt": "", "did": null, "bfont": "", "bsize": 0}],
        "flds": [field_json("Front", 0), field_json("Back", 1), field_json("Source", 2)],
        "css": CSS, "latexPre": LATEX_PRE, "latexPost": "\\end{document}", "latexsvg": false,
        "req": [[0, "any", [0]]], "tags": [], "vers": []
    });
    let cloze = json!({
        "id": CLOZE_MODEL_ID, "name": "Tern Cloze", "type": 1, "mod": now, "usn": -1,
        "sortf": 0, "did": did,
        "tmpls": [{"name": "Cloze", "ord": 0,
            "qfmt": "{{cloze:Text}}",
            "afmt": "{{cloze:Text}}<br>\n{{Back Extra}}\n{{#Source}}<div class=\"source\">{{Source}}</div>{{/Source}}",
            "bqfmt": "", "bafmt": "", "did": null, "bfont": "", "bsize": 0}],
        "flds": [field_json("Text", 0), field_json("Back Extra", 1), field_json("Source", 2)],
        "css": CSS, "latexPre": LATEX_PRE, "latexPost": "\\end{document}", "latexsvg": false,
        "tags": [], "vers": []
    });
    json!({ BASIC_MODEL_ID.to_string(): basic, CLOZE_MODEL_ID.to_string(): cloze })
}

fn deck_json(id: i64, name: &str, now: i64) -> serde_json::Value {
    json!({
        "id": id, "mod": now, "name": name, "usn": -1,
        "lrnToday": [0, 0], "revToday": [0, 0], "newToday": [0, 0], "timeToday": [0, 0],
        "collapsed": false, "browserCollapsed": false, "desc": "", "dyn": 0, "conf": 1,
        "extendNew": 0, "extendRev": 0
    })
}

fn dconf_json() -> serde_json::Value {
    json!({"1": {
        "id": 1, "name": "Default", "replayq": true, "timer": 0, "maxTaken": 60, "usn": 0,
        "mod": 0, "autoplay": true, "dyn": false,
        "lapse": {"delays": [10], "mult": 0, "minInt": 1, "leechFails": 8, "leechAction": 0},
        "rev": {"perDay": 200, "ease4": 1.3, "fuzz": 0.05, "minSpace": 1, "ivlFct": 1,
                "maxIvl": 36500, "bury": false, "hardFactor": 1.2},
        "new": {"delays": [1, 10], "ints": [1, 4, 7], "initialFactor": 2500, "order": 1,
                "perDay": 20, "bury": false, "separate": true}
    }})
}

fn conf_json(did: i64) -> serde_json::Value {
    json!({
        "activeDecks": [did], "curDeck": did, "newSpread": 0, "collapseTime": 1200,
        "timeLim": 0, "estTimes": true, "dueCounts": true, "curModel": BASIC_MODEL_ID.to_string(),
        "nextPos": 1, "sortType": "noteFld", "sortBackwards": false, "addToCur": true
    })
}

fn sql(e: rusqlite::Error) -> String {
    format!("Tern could not write the Anki collection: {e}")
}

/// Writes the collection database at `db`.
fn write_collection(pkg: &Package, db: &Path) -> Result<Summary, String> {
    let _ = std::fs::remove_file(db);
    let conn = Connection::open(db).map_err(sql)?;
    conn.execute_batch(SCHEMA).map_err(sql)?;
    let now = pkg.created;
    let ms = now * 1000;
    let crt = now.div_euclid(DAY) * DAY;
    let did = deck_id(&pkg.deck);
    let decks = json!({
        "1": deck_json(1, "Default", now),
        did.to_string(): deck_json(did, &pkg.deck, now),
    });
    let mut all_tags: std::collections::BTreeSet<String> = Default::default();
    for n in &pkg.notes {
        all_tags.extend(n.tags.iter().cloned());
    }
    let tags_json: serde_json::Map<String, serde_json::Value> =
        all_tags.iter().map(|t| (t.clone(), json!(0))).collect();
    conn.execute(
        "INSERT INTO col VALUES (1, ?1, ?2, ?3, 11, 0, 0, 0, ?4, ?5, ?6, ?7, ?8)",
        params![
            crt,
            ms,
            ms,
            conf_json(did).to_string(),
            models_json(did, now).to_string(),
            decks.to_string(),
            dconf_json().to_string(),
            serde_json::Value::Object(tags_json).to_string(),
        ],
    )
    .map_err(sql)?;
    let mut summary = Summary {
        decks: vec![pkg.deck.clone()],
        models: vec!["Tern Basic".into(), "Tern Cloze".into()],
        ..Summary::default()
    };
    let mut card_id = ms;
    let mut new_pos = 0i64;
    for (i, n) in pkg.notes.iter().enumerate() {
        let nid = ms + i as i64;
        let mid = match n.kind {
            NoteKind::Basic => BASIC_MODEL_ID,
            NoteKind::Cloze => CLOZE_MODEL_ID,
        };
        let tags = if n.tags.is_empty() {
            String::new()
        } else {
            format!(" {} ", n.tags.join(" "))
        };
        conn.execute(
            "INSERT INTO notes VALUES (?1, ?2, ?3, ?4, -1, ?5, ?6, ?7, ?8, 0, '')",
            params![
                nid,
                guid(&n.guid_seed),
                mid,
                now,
                tags,
                n.fields.join("\u{1f}"),
                strip_html(&n.fields[0]),
                checksum(&n.fields[0]),
            ],
        )
        .map_err(sql)?;
        summary.notes += 1;
        let ords: Vec<i64> = match n.kind {
            NoteKind::Basic => vec![0],
            NoteKind::Cloze => {
                let mut v: Vec<i64> = n
                    .clozes
                    .iter()
                    .filter(|c| **c >= 1)
                    .map(|c| i64::from(*c) - 1)
                    .collect();
                if v.is_empty() {
                    v.push(0);
                }
                v
            }
        };
        for ord in ords {
            // type, queue, due, ivl, factor, reps, lapses
            let (ty, mut queue, due, ivl, factor, reps, lapses) = match n.review {
                Some(r) => (
                    2,
                    2,
                    ((r.due - crt).div_euclid(DAY)).max(0),
                    r.interval.max(1),
                    r.factor.clamp(1300, 9999),
                    r.reps,
                    r.lapses,
                ),
                None => {
                    new_pos += 1;
                    (0, 0, new_pos, 0, 0, 0, 0)
                }
            };
            if n.suspended {
                queue = -1;
            }
            card_id += 1;
            conn.execute(
                "INSERT INTO cards VALUES (?1, ?2, ?3, ?4, ?5, -1, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, 0, 0, 0, '')",
                params![card_id, nid, did, ord, now, ty, queue, due, ivl, factor, reps, lapses],
            )
            .map_err(sql)?;
            summary.cards += 1;
        }
    }
    drop(conn);
    Ok(summary)
}

/// Writes `pkg` as an `.apkg` at `path`.
pub fn write_apkg(pkg: &Package, path: &Path) -> Result<Summary, String> {
    let db = path.with_extension("anki2.tmp");
    let summary = write_collection(pkg, &db)?;
    let bytes = std::fs::read(&db).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&db);
    let bytes = bytes?;
    let file = std::fs::File::create(path)
        .map_err(|e| format!("Tern could not write {}: {e}", path.display()))?;
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let io = |e: std::io::Error| e.to_string();
    z.start_file("collection.anki2", opts)
        .map_err(|e| e.to_string())?;
    z.write_all(&bytes).map_err(io)?;
    z.start_file("media", opts).map_err(|e| e.to_string())?;
    z.write_all(b"{}").map_err(io)?;
    z.finish().map_err(|e| e.to_string())?;
    Ok(summary)
}

/// Opens an `.apkg` and reports what it holds, checking it is shaped the
/// way Anki expects: a zip with `collection.anki2` and `media`, the
/// schema-11 tables, and note types and decks that parse.
pub fn inspect_apkg(path: &Path) -> Result<Summary, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(file).map_err(|e| format!("Not a zip: {e}"))?;
    let mut media = String::new();
    z.by_name("media")
        .map_err(|_| "No media entry.".to_string())?
        .read_to_string(&mut media)
        .map_err(|e| e.to_string())?;
    serde_json::from_str::<serde_json::Value>(&media).map_err(|e| format!("media: {e}"))?;
    let mut bytes = Vec::new();
    z.by_name("collection.anki2")
        .map_err(|_| "No collection.anki2 entry.".to_string())?
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let tmp = path.with_extension("inspect.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    let r = (|| {
        let conn = Connection::open(&tmp).map_err(|e| e.to_string())?;
        let q = |e: rusqlite::Error| e.to_string();
        let ver: i64 = conn
            .query_row("SELECT ver FROM col", [], |r| r.get(0))
            .map_err(q)?;
        if ver != 11 {
            return Err(format!("Unexpected schema version {ver}."));
        }
        let (models, decks): (String, String) = conn
            .query_row("SELECT models, decks FROM col", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(q)?;
        let models: serde_json::Value = serde_json::from_str(&models).map_err(|e| e.to_string())?;
        let decks: serde_json::Value = serde_json::from_str(&decks).map_err(|e| e.to_string())?;
        let names = |v: &serde_json::Value| -> Vec<String> {
            v.as_object()
                .map(|o| {
                    o.values()
                        .filter_map(|m| m["name"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let notes: i64 = conn
            .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
            .map_err(q)?;
        let cards: i64 = conn
            .query_row("SELECT count(*) FROM cards", [], |r| r.get(0))
            .map_err(q)?;
        // Every card belongs to a note, every note to a known note type.
        let orphans: i64 = conn
            .query_row(
                "SELECT count(*) FROM cards WHERE nid NOT IN (SELECT id FROM notes)",
                [],
                |r| r.get(0),
            )
            .map_err(q)?;
        if orphans > 0 {
            return Err(format!("{orphans} cards have no note."));
        }
        let mut st = conn.prepare("SELECT mid, flds FROM notes").map_err(q)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(q)?;
        for row in rows {
            let (mid, flds) = row.map_err(q)?;
            let m = &models[mid.to_string()];
            let want = m["flds"].as_array().map(Vec::len).unwrap_or(0);
            let have = flds.split('\u{1f}').count();
            if want == 0 || want != have {
                return Err(format!(
                    "A note has {have} fields; its note type has {want}."
                ));
            }
        }
        let mut deck_names = names(&decks);
        deck_names.retain(|d| d != "Default");
        let mut model_names = names(&models);
        model_names.sort();
        Ok(Summary {
            notes: notes as usize,
            cards: cards as usize,
            decks: deck_names,
            models: model_names,
        })
    })();
    let _ = std::fs::remove_file(&tmp);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg() -> Package {
        Package {
            deck: "Tern::Graphs".into(),
            created: 1_760_000_000,
            notes: vec![
                Note {
                    kind: NoteKind::Basic,
                    fields: [
                        "What is a <b>graph</b>?".into(),
                        "Vertices joined by edges.".into(),
                        "algo.pdf p. 3".into(),
                    ],
                    tags: vec!["graphs".into()],
                    guid_seed: "k1".into(),
                    clozes: vec![],
                    review: Some(ReviewState {
                        interval: 10,
                        factor: 2500,
                        reps: 4,
                        lapses: 0,
                        due: 1_760_000_000 + 5 * DAY,
                    }),
                    suspended: false,
                },
                Note {
                    kind: NoteKind::Cloze,
                    fields: [
                        "{{c1::Dijkstra}} needs {{c2::non-negative}} weights.".into(),
                        String::new(),
                        String::new(),
                    ],
                    tags: vec!["graphs".into(), "shortest-paths".into()],
                    guid_seed: "k2".into(),
                    clozes: vec![1, 2],
                    review: None,
                    suspended: true,
                },
            ],
        }
    }

    #[test]
    fn writes_a_package_anki_can_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deck.apkg");
        let wrote = write_apkg(&pkg(), &path).unwrap();
        assert_eq!((wrote.notes, wrote.cards), (2, 3));
        assert!(
            !dir.path().join("deck.anki2.tmp").exists(),
            "temp file cleaned"
        );
        let read = inspect_apkg(&path).unwrap();
        assert_eq!((read.notes, read.cards), (2, 3));
        assert_eq!(read.decks, vec!["Tern::Graphs"]);
        assert_eq!(read.models, vec!["Tern Basic", "Tern Cloze"]);

        // Look inside: schedule, tags, sort field, and checksum.
        let mut z = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let names: Vec<String> = z.file_names().map(str::to_owned).collect();
        assert!(names.contains(&"collection.anki2".to_string()));
        assert!(names.contains(&"media".to_string()));
        let mut bytes = Vec::new();
        z.by_name("collection.anki2")
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        let db = dir.path().join("c.anki2");
        std::fs::write(&db, bytes).unwrap();
        let c = Connection::open(&db).unwrap();
        let (sfld, csum, tags): (String, i64, String) = c
            .query_row(
                "SELECT sfld, csum, tags FROM notes ORDER BY id LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(sfld, "What is a graph?");
        assert_eq!(csum, checksum("What is a graph?"));
        assert_eq!(tags, " graphs ");
        let (ty, queue, due, ivl, factor): (i64, i64, i64, i64, i64) = c
            .query_row(
                "SELECT type, queue, due, ivl, factor FROM cards ORDER BY id LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!((ty, queue, ivl, factor), (2, 2, 10, 2500));
        let crt: i64 = c
            .query_row("SELECT crt FROM col", [], |r| r.get(0))
            .unwrap();
        assert_eq!(due, (1_760_000_000 + 5 * DAY - crt) / DAY);
        let ords: Vec<(i64, i64)> = c
            .prepare("SELECT ord, queue FROM cards WHERE type = 0 ORDER BY ord")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            ords,
            vec![(0, -1), (1, -1)],
            "one card per cloze, suspended"
        );
        let col_tags: String = c
            .query_row("SELECT tags FROM col", [], |r| r.get(0))
            .unwrap();
        assert!(col_tags.contains("shortest-paths"));
    }

    #[test]
    fn helpers_are_stable() {
        // Anki's checksum of "hello": sha1 aaf4c61d… → 0xaaf4c61d.
        assert_eq!(checksum("<i>hello</i>"), 0xaaf4c61d);
        assert_eq!(guid("k1"), guid("k1"));
        assert_ne!(guid("k1"), guid("k2"));
        assert!(deck_id("Tern") > 1);
        assert_eq!(strip_html("a &amp; <br>b"), "a & b");
        assert!(inspect_apkg(Path::new("/nonexistent.apkg")).is_err());
    }
}
