//! Checksums for the image store and content hashes for the repo templates.
//!
//! Two different questions get answered here with the same primitive:
//!
//! - *Is this image file intact?* A truncated or half-written qcow2 is the
//!   failure mode that actually happens, and the answer has to be cheap enough
//!   to compute over tens of gigabytes. CRC-32 reads at memory speed; a
//!   cryptographic digest would turn `vm doctor` into a minute-long command for
//!   no benefit, because nothing here defends against a forged image.
//! - *Was this image built from the templates currently in the repo?* That is
//!   the currency model from plan decision 4: the manifest records the hash of
//!   the template tree the image came from, and a mismatch means stale.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Read buffer for streaming a multi-gigabyte image through the hasher.
const CHUNK: usize = 1 << 20;

/// Hash a byte slice into the `crc32:` form the manifest stores.
pub fn checksum_bytes(data: &[u8]) -> String {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(data);
    format!("crc32:{:08x}", hasher.finalize())
}

/// Stream a file through the hasher without holding it in memory.
pub fn checksum_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buf = vec![0_u8; CHUNK];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("crc32:{:08x}", hasher.finalize()))
}

/// Content hash of a template tree, as `(relative path, contents)` pairs.
///
/// The result is independent of the order the caller collected the files in and
/// changes if any path or any byte changes. Line endings are normalized to LF
/// first: every file under `vm/` is text, the repo is LF everywhere, and a
/// checkout that disagreed would otherwise mark every image stale for a
/// difference no builder can see.
pub fn template_hash(entries: &[(String, Vec<u8>)]) -> String {
    let mut sorted: Vec<&(String, Vec<u8>)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = crc32fast::Hasher::new();
    for (name, content) in sorted {
        let normalized = normalize_newlines(content);
        // Length-prefix both halves so that renaming a file cannot produce the
        // same byte stream as editing its contents.
        hasher.update(&u64::try_from(name.len()).unwrap_or(u64::MAX).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update(
            &u64::try_from(normalized.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        hasher.update(&normalized);
    }
    format!("crc32:{:08x}", hasher.finalize())
}

/// Drop carriage returns that precede a line feed.
fn normalize_newlines(content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len());
    let mut i = 0;
    while i < content.len() {
        if content[i] == b'\r' && content.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(content[i]);
        i += 1;
    }
    out
}

/// Collect every file under `dir`, recursively, as `(relative path, contents)`
/// with forward-slash separators.
///
/// This is the effectful half; [`template_hash`] is the part with the rules in
/// it and the part the tests exercise.
pub fn read_tree(dir: &Path) -> io::Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    collect(dir, &PathBuf::new(), &mut out)?;
    Ok(out)
}

fn collect(root: &Path, prefix: &Path, out: &mut Vec<(String, Vec<u8>)>) -> io::Result<()> {
    let dir = root.join(prefix);
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let rel = prefix.join(&name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect(root, &rel, out)?;
        } else if file_type.is_file() {
            let contents = std::fs::read(entry.path())?;
            let key = rel.to_string_lossy().replace('\\', "/");
            out.push((key, contents));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, body: &str) -> (String, Vec<u8>) {
        (name.to_owned(), body.as_bytes().to_vec())
    }

    #[test]
    fn checksums_are_stable_and_tagged() {
        assert_eq!(checksum_bytes(b""), "crc32:00000000");
        assert_eq!(checksum_bytes(b"123456789"), "crc32:cbf43926");
    }

    #[test]
    fn template_hash_ignores_the_order_files_were_collected_in() {
        let a = vec![entry("a.hcl", "one"), entry("b/c.xml", "two")];
        let b = vec![entry("b/c.xml", "two"), entry("a.hcl", "one")];
        assert_eq!(template_hash(&a), template_hash(&b));
    }

    #[test]
    fn template_hash_changes_when_contents_change() {
        let before = vec![entry("a.hcl", "one")];
        let after = vec![entry("a.hcl", "onf")];
        assert_ne!(template_hash(&before), template_hash(&after));
    }

    #[test]
    fn template_hash_changes_when_a_file_is_renamed_or_added() {
        let base = vec![entry("a.hcl", "one")];
        let renamed = vec![entry("b.hcl", "one")];
        let added = vec![entry("a.hcl", "one"), entry("b.hcl", "")];
        assert_ne!(template_hash(&base), template_hash(&renamed));
        assert_ne!(template_hash(&base), template_hash(&added));
    }

    #[test]
    fn moving_a_character_across_the_name_content_boundary_is_not_a_collision() {
        let left = vec![entry("ab", "cd")];
        let right = vec![entry("a", "bcd")];
        assert_ne!(template_hash(&left), template_hash(&right));
    }

    #[test]
    fn a_crlf_checkout_hashes_the_same_as_an_lf_one() {
        let lf = vec![entry("a.sh", "line one\nline two\n")];
        let crlf = vec![entry("a.sh", "line one\r\nline two\r\n")];
        assert_eq!(template_hash(&lf), template_hash(&crlf));
        // A bare carriage return is content, not a line ending, and survives.
        let cr = vec![entry("a.sh", "line one\rline two\r")];
        assert_ne!(template_hash(&lf), template_hash(&cr));
    }

    #[test]
    fn an_empty_tree_still_hashes() {
        assert_eq!(template_hash(&[]), "crc32:00000000");
    }
}
