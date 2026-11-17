//! The table of day articles: for each Wikipedia, the title of its article
//! on each day of the year ("January 1", "1er janvier", "1月1日"), from the
//! Wikidata items those articles share.
//!
//! One line per wiki, by its subdomain ("en", "zh-classical"), then a tab
//! and the 366 titles from January 1 to December 31 (February 29
//! included), tab-separated, empty where the wiki has no such article;
//! lines in subdomain order; compressed with zstd.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

const MONTHS: [(&str, u32); 12] = [
    ("January", 31),
    ("February", 29),
    ("March", 31),
    ("April", 30),
    ("May", 31),
    ("June", 30),
    ("July", 31),
    ("August", 31),
    ("September", 30),
    ("October", 31),
    ("November", 30),
    ("December", 31),
];

/// Wikimedia asks tools to say who they are and where to find out more.
const USER_AGENT: &str = concat!(
    "xwa-tool/",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

/// English Wikipedia's titles for the days, in order.
fn english_days() -> Vec<String> {
    MONTHS
        .iter()
        .flat_map(|(m, n)| (1..=*n).map(move |d| format!("{m} {d}")))
        .collect()
}

pub fn write(root: &Path) -> Result<(), String> {
    let days = english_days();
    // The items English Wikipedia's day articles are about, and every
    // Wikipedia's article about each.
    let values: Vec<String> = days.iter().map(|d| format!("\"{d}\"@en")).collect();
    let query = format!(
        "SELECT ?en ?site ?title WHERE {{ VALUES ?en {{ {} }} \
         ?enArticle schema:about ?item; schema:isPartOf <https://en.wikipedia.org/>; schema:name ?en. \
         ?article schema:about ?item; schema:isPartOf ?site; schema:name ?title. \
         FILTER(STRENDS(STR(?site), \".wikipedia.org/\")) }}",
        values.join(" ")
    );
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(USER_AGENT)
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .into();
    eprintln!("asking Wikidata for the day articles of every Wikipedia");
    let body = agent
        .post("https://query.wikidata.org/sparql")
        .header("Accept", "application/sparql-results+json")
        .send_form([("query", query.as_str())])
        .map_err(|e| format!("Wikidata: {e}"))?
        .body_mut()
        .with_config()
        .limit(256 << 20)
        .read_to_string()
        .map_err(|e| format!("Wikidata: {e}"))?;
    let json: Value = serde_json::from_str(&body).map_err(|e| format!("Wikidata: {e}"))?;
    let rows = json["results"]["bindings"]
        .as_array()
        .ok_or("Wikidata: no results")?;

    let index: BTreeMap<&str, usize> = days
        .iter()
        .enumerate()
        .map(|(i, d)| (d.as_str(), i))
        .collect();
    let mut wikis: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows {
        let field = |k: &str| row[k]["value"].as_str().unwrap_or_default();
        let (Some(&day), Some(wiki)) = (
            index.get(field("en")),
            field("site")
                .strip_prefix("https://")
                .and_then(|s| s.strip_suffix(".wikipedia.org/")),
        ) else {
            continue;
        };
        let title = field("title");
        if title.is_empty() || title.contains(['\t', '\n']) {
            continue;
        }
        let titles = wikis
            .entry(wiki.to_string())
            .or_insert_with(|| vec![String::new(); days.len()]);
        titles[day] = title.to_string();
    }
    if wikis
        .get("en")
        .is_none_or(|t| t.iter().any(String::is_empty))
    {
        return Err("Wikidata's answer lacks English Wikipedia's days".into());
    }
    let mut table = String::new();
    for (wiki, titles) in &wikis {
        table.push_str(wiki);
        for t in titles {
            table.push('\t');
            table.push_str(t);
        }
        table.push('\n');
    }
    let packed = zstd::encode_all(table.as_bytes(), 19).map_err(|e| e.to_string())?;
    let out = root.join("crates/xwa-convert/data/days.tsv.zst");
    std::fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&out, &packed).map_err(|e| format!("{}: {e}", out.display()))?;
    let titles = wikis.values().flatten().filter(|t| !t.is_empty()).count();
    eprintln!(
        "wrote {} wikis, {titles} titles, {} bytes to {}",
        wikis.len(),
        packed.len(),
        out.display()
    );
    Ok(())
}
