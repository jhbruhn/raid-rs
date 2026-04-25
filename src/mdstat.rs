/*!
    Parse /proc/mdstat into a structured view.

    /proc/mdstat is the kernel's live view of every assembled md array.
    It is world-readable and updated atomically, so a single open is the
    cheapest way to learn the state of every array on a host — including
    rebuild/resync progress, which `mdadm --detail` does not expose
    directly.

    This module is a pure parser plus a thin `read_mdstat()` wrapper.
    `parse_mdstat(&str)` is total: it never panics on malformed input,
    it discards lines it does not understand, and it returns whatever it
    could recover. Callers should always tolerate `arrays.is_empty()`.

    This program is free software; you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation; either version 2 of the License, or
    (at your option) any later version.
*/

use std::{fs, io};

/// Top-level snapshot of /proc/mdstat at one instant.
#[derive(Debug, Clone, PartialEq)]
pub struct MdStat {
    /// RAID personalities the kernel has loaded, e.g. `["raid0","raid1","raid5"]`.
    pub personalities: Vec<String>,
    pub arrays: Vec<MdArray>,
}

/// One md device as listed in mdstat.
#[derive(Debug, Clone, PartialEq)]
pub struct MdArray {
    /// Kernel name, no `/dev/` prefix. e.g. `md127`.
    pub name: String,
    /// Whether the kernel reports this array as `active` (vs `inactive`).
    pub active: bool,
    /// Personality string, e.g. `raid5`. None for arrays with no personality
    /// (very rarely seen on stacked / partial assemblies).
    pub level: Option<String>,
    /// Members in the order mdstat lists them. Order is not stable across
    /// runs and is not the slot order — use `MdMember::role` for that.
    pub members: Vec<MdMember>,
    /// "N blocks" from the second line. None if the line is absent.
    pub blocks: Option<u64>,
    /// "super 1.2" if reported.
    pub super_version: Option<String>,
    /// "512k chunk" → 512. None for raid1/raid0-without-chunk.
    pub chunk_kib: Option<u64>,
    /// "algorithm 2" or similar — left as a free-form string because levels
    /// have different layout vocabularies (left-symmetric vs near=2 vs ...).
    pub layout: Option<String>,
    /// "[N/M]" — total slots vs working slots. `(raid_disks, working_disks)`.
    /// None for personalities that don't print one (raid0).
    pub disks_status: Option<(usize, usize)>,
    /// "[UUU]" / "[U_U]" — raw glyphs as printed. The length is the
    /// authoritative `raid_disks` and the count of `U` is `working_disks`.
    pub status_glyphs: Option<String>,
    /// Active recovery / resync / check / reshape, or None if idle.
    pub action: Option<MdAction>,
    /// Bitmap line, if any.
    pub bitmap: Option<MdBitmap>,
}

/// One member of an md array as listed in mdstat.
#[derive(Debug, Clone, PartialEq)]
pub struct MdMember {
    /// Kernel device name with no /dev/ prefix, e.g. `sda1` or `loop10p1`.
    pub kernel_name: String,
    /// Slot index from the md superblock. May be ≥ `raid_disks` when a
    /// previously-occupied slot was replaced and the new disk got the next
    /// free slot rather than reusing the gap.
    pub role: usize,
    pub failed: bool,        // (F)
    pub spare: bool,         // (S)
    pub journal: bool,       // (J)
    pub replacement: bool,   // (R)
    pub write_mostly: bool,  // (W)
}

/// Active sync work running on an array.
#[derive(Debug, Clone, PartialEq)]
pub struct MdAction {
    pub kind: MdActionKind,
    /// 0.0 .. 100.0 inclusive.
    pub percent: f32,
    /// Blocks completed and total, parsed from `(done/total)`.
    pub done_blocks: u64,
    pub total_blocks: u64,
    /// Throughput in KiB/s as the kernel reports it.
    pub speed_kib_s: u64,
    /// ETA in minutes from `finish=NN.Nmin`.
    pub finish_minutes: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdActionKind {
    Resync,
    Recovery,
    Check,
    Repair,
    Reshape,
}

/// Bitmap line, e.g. `bitmap: 0/1 pages [0KB], 65536KB chunk`.
#[derive(Debug, Clone, PartialEq)]
pub struct MdBitmap {
    pub pages_used: u64,
    pub pages_total: u64,
    pub chunk_kib: Option<u64>,
}

/// Read /proc/mdstat from the running kernel.
pub fn read_mdstat() -> io::Result<MdStat> {
    let raw = fs::read_to_string("/proc/mdstat")?;
    Ok(parse_mdstat(&raw))
}

/// Parse mdstat text. Total: malformed lines are silently dropped.
pub fn parse_mdstat(raw: &str) -> MdStat {
    let mut personalities: Vec<String> = Vec::new();
    let mut arrays: Vec<MdArray> = Vec::new();

    // mdstat groups lines per array: a header (`mdN : ...`), then one or
    // more continuation lines that begin with whitespace. We collect each
    // group and parse it as a unit.
    let mut group: Vec<&str> = Vec::new();

    let flush = |group: &mut Vec<&str>, arrays: &mut Vec<MdArray>| {
        if let Some(arr) = parse_array_group(group) {
            arrays.push(arr);
        }
        group.clear();
    };

    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("Personalities :") {
            personalities = rest
                .split_whitespace()
                .map(|t| t.trim_matches(|c| c == '[' || c == ']').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            continue;
        }
        if line.starts_with("unused devices:") {
            continue;
        }
        if line.trim().is_empty() {
            flush(&mut group, &mut arrays);
            continue;
        }
        if !line.starts_with(char::is_whitespace) && !group.is_empty() {
            flush(&mut group, &mut arrays);
        }
        group.push(line);
    }
    flush(&mut group, &mut arrays);

    MdStat { personalities, arrays }
}

fn parse_array_group(lines: &[&str]) -> Option<MdArray> {
    let header = lines.first()?;
    let (name, rest) = header.split_once(" : ")?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return None;
    }

    let mut tokens = rest.split_whitespace();
    let active_token = tokens.next()?;
    let active = active_token == "active";
    // Some kernels emit "active (auto-read-only)" or "active (read-only)";
    // skip parenthesised qualifiers until we hit the personality.
    let mut level_token = tokens.next();
    while let Some(tok) = level_token {
        if tok.starts_with('(') {
            level_token = tokens.next();
        } else {
            break;
        }
    }
    let level = level_token.map(|s| s.to_string());

    let members: Vec<MdMember> = tokens.filter_map(parse_member_token).collect();

    let mut arr = MdArray {
        name,
        active,
        level,
        members,
        blocks: None,
        super_version: None,
        chunk_kib: None,
        layout: None,
        disks_status: None,
        status_glyphs: None,
        action: None,
        bitmap: None,
    };

    for cont in lines.iter().skip(1) {
        let trimmed = cont.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(bm) = parse_bitmap_line(trimmed) {
            arr.bitmap = Some(bm);
            continue;
        }
        if let Some(act) = parse_action_line(trimmed) {
            arr.action = Some(act);
            continue;
        }
        // Otherwise treat as the metadata line ("N blocks super 1.2 ... [N/M] [UUU]").
        apply_metadata_line(&mut arr, trimmed);
    }

    Some(arr)
}

/// Parse a single member token like `loop10p1[3]`, `sda1[2](F)`, `nvme0n1[4](S)(W)`.
fn parse_member_token(tok: &str) -> Option<MdMember> {
    let lb = tok.find('[')?;
    let kernel_name = tok[..lb].to_string();
    if kernel_name.is_empty() {
        return None;
    }
    let after = &tok[lb + 1..];
    let rb = after.find(']')?;
    let role: usize = after[..rb].parse().ok()?;
    let flags = &after[rb + 1..];

    let mut m = MdMember {
        kernel_name,
        role,
        failed: false,
        spare: false,
        journal: false,
        replacement: false,
        write_mostly: false,
    };
    // Flags are zero or more `(X)` suffixes concatenated with no spaces.
    let mut rest = flags;
    while let Some(open) = rest.find('(') {
        let after = &rest[open + 1..];
        let close = match after.find(')') {
            Some(c) => c,
            None => break,
        };
        match &after[..close] {
            "F" => m.failed = true,
            "S" => m.spare = true,
            "J" => m.journal = true,
            "R" => m.replacement = true,
            "W" => m.write_mostly = true,
            _ => {}
        }
        rest = &after[close + 1..];
    }
    Some(m)
}

fn apply_metadata_line(arr: &mut MdArray, line: &str) {
    // "4185088 blocks super 1.2 level 5, 512k chunk, algorithm 2 [3/3] [UUU]"
    if let Some(blocks) = parse_leading_blocks(line) {
        arr.blocks = Some(blocks);
    }
    if let Some(sv) = extract_after(line, "super ") {
        arr.super_version = Some(first_token(sv).to_string());
    }
    if let Some(chunk) = parse_chunk_kib(line) {
        arr.chunk_kib = Some(chunk);
    }
    if let Some(algo) = extract_after(line, "algorithm ") {
        arr.layout = Some(format!("algorithm {}", first_token(algo)));
    }
    if let Some((total, working)) = parse_disks_status(line) {
        arr.disks_status = Some((total, working));
    }
    if let Some(g) = parse_glyphs(line) {
        arr.status_glyphs = Some(g);
    }
}

fn parse_leading_blocks(line: &str) -> Option<u64> {
    let mut it = line.split_whitespace();
    let first = it.next()?;
    let second = it.next()?;
    if second != "blocks" {
        return None;
    }
    first.parse().ok()
}

fn parse_chunk_kib(line: &str) -> Option<u64> {
    // "...512k chunk..." — find "k chunk" and walk back for the integer.
    let needle = "k chunk";
    let pos = line.find(needle)?;
    let head = &line[..pos];
    let num_start = head.rfind(|c: char| !c.is_ascii_digit()).map(|i| i + 1).unwrap_or(0);
    head[num_start..].parse().ok()
}

fn parse_disks_status(line: &str) -> Option<(usize, usize)> {
    // First "[N/M]" bracket — must come *before* the glyph bracket.
    let lb = line.find('[')?;
    let rb_off = line[lb..].find(']')?;
    let inner = &line[lb + 1..lb + rb_off];
    let (a, b) = inner.split_once('/')?;
    let total: usize = a.parse().ok()?;
    let working: usize = b.parse().ok()?;
    Some((total, working))
}

fn parse_glyphs(line: &str) -> Option<String> {
    // Last bracketed run made of `U` / `_` only.
    let mut start = None;
    for (i, c) in line.char_indices() {
        if c == '[' {
            start = Some(i + 1);
        } else if c == ']' {
            if let Some(s) = start {
                let inner = &line[s..i];
                if !inner.is_empty() && inner.chars().all(|c| c == 'U' || c == '_') {
                    return Some(format!("[{}]", inner));
                }
                start = None;
            }
        }
    }
    None
}

fn parse_action_line(line: &str) -> Option<MdAction> {
    // Examples:
    //   "[==>..................]  recovery = 25.5% (50000/200000) finish=2.3min speed=12345K/sec"
    //   "[>....................]  resync =  0.1% (1024/4185088) finish=33.0min speed=2048K/sec"
    //   "[=====>...............]  check  = 27.0% (...)"
    //   "       resync=DELAYED" / "PENDING" — skip, no measurable progress.
    //
    // Note: the leading "[==>...]" bar contains literal `=` characters, so
    // we must locate the kind keyword first and only look for the `=`
    // separator *after* it.
    let kinds = [
        ("recovery", MdActionKind::Recovery),
        ("resync", MdActionKind::Resync),
        ("reshape", MdActionKind::Reshape),
        ("check", MdActionKind::Check),
        ("repair", MdActionKind::Repair),
    ];
    let (kw_pos, kw_len, kind) = kinds
        .iter()
        .find_map(|(kw, k)| line.find(kw).map(|p| (p, kw.len(), *k)))?;
    let after_kw = &line[kw_pos + kw_len..];
    let eq_off = after_kw.find('=')?;
    let after_eq = &after_kw[eq_off + 1..];

    // percent
    let pct_end = after_eq.find('%')?;
    let percent: f32 = after_eq[..pct_end].trim().parse().ok()?;

    // (done/total)
    let lp = after_eq.find('(')?;
    let rp = after_eq[lp..].find(')')?;
    let inner = &after_eq[lp + 1..lp + rp];
    let (done_str, total_str) = inner.split_once('/')?;
    let done_blocks: u64 = done_str.trim().parse().ok()?;
    let total_blocks: u64 = total_str.trim().parse().ok()?;

    let finish_minutes = extract_after(line, "finish=")
        .and_then(parse_leading_decimal)
        .unwrap_or(0.0);

    let speed_kib_s = extract_after(line, "speed=")
        .and_then(|s| {
            let n: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
            n.parse::<u64>().ok()
        })
        .unwrap_or(0);

    Some(MdAction {
        kind,
        percent,
        done_blocks,
        total_blocks,
        speed_kib_s,
        finish_minutes,
    })
}

fn parse_leading_decimal(s: &str) -> Option<f32> {
    let n: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    n.parse::<f32>().ok()
}

fn parse_bitmap_line(line: &str) -> Option<MdBitmap> {
    // "bitmap: 0/1 pages [0KB], 65536KB chunk"
    let rest = line.strip_prefix("bitmap:")?.trim();
    let (pages_part, tail) = rest.split_once(' ')?;
    let (used, total) = pages_part.split_once('/')?;
    let pages_used: u64 = used.parse().ok()?;
    let pages_total: u64 = total.parse().ok()?;
    let chunk_kib = extract_after(tail, ",").and_then(|s| {
        let s = s.trim();
        let n: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        n.parse::<u64>().ok()
    });
    Some(MdBitmap { pages_used, pages_total, chunk_kib })
}

fn extract_after<'a>(haystack: &'a str, needle: &str) -> Option<&'a str> {
    let i = haystack.find(needle)?;
    Some(&haystack[i + needle.len()..])
}

fn first_token(s: &str) -> &str {
    s.split(|c: char| c.is_whitespace() || c == ',').next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE_RAID5: &str = "\
Personalities : [raid0] [raid1] [raid6] [raid5] [raid4] [raid10]
md127 : active raid5 loop10p1[3] loop12p1[1] loop11p1[0]
      4185088 blocks super 1.2 level 5, 512k chunk, algorithm 2 [3/3] [UUU]
      bitmap: 0/1 pages [0KB], 65536KB chunk

unused devices: <none>
";

    const RECOVERING_RAID1: &str = "\
Personalities : [raid1]
md0 : active raid1 sdb1[2] sda1[0]
      1048512 blocks super 1.2 [2/1] [U_]
      [=====>...............]  recovery = 27.5% (288768/1048512) finish=2.3min speed=2048K/sec

unused devices: <none>
";

    const DEGRADED_NO_REBUILD: &str = "\
Personalities : [raid5]
md1 : active raid5 sdc1[2](F) sdb1[1] sda1[0]
      8388480 blocks super 1.2 level 5, 64k chunk, algorithm 2 [3/2] [UU_]

unused devices: <none>
";

    const RAID0_NO_GLYPHS: &str = "\
Personalities : [raid0]
md2 : active raid0 sdd1[1] sde1[0]
      8388352 blocks super 1.2 512k chunks

unused devices: <none>
";

    const TWO_ARRAYS: &str = "\
Personalities : [raid1] [raid5]
md0 : active raid1 sda1[0] sdb1[1]
      1048512 blocks super 1.2 [2/2] [UU]
md1 : active raid5 sda2[0] sdb2[1] sdc2[2]
      4185088 blocks super 1.2 level 5, 512k chunk, algorithm 2 [3/3] [UUU]

unused devices: <none>
";

    #[test]
    fn parses_idle_raid5_with_non_dense_roles() {
        let s = parse_mdstat(IDLE_RAID5);
        assert_eq!(s.personalities, vec!["raid0", "raid1", "raid6", "raid5", "raid4", "raid10"]);
        assert_eq!(s.arrays.len(), 1);
        let a = &s.arrays[0];
        assert_eq!(a.name, "md127");
        assert!(a.active);
        assert_eq!(a.level.as_deref(), Some("raid5"));
        assert_eq!(a.blocks, Some(4185088));
        assert_eq!(a.super_version.as_deref(), Some("1.2"));
        assert_eq!(a.chunk_kib, Some(512));
        assert_eq!(a.layout.as_deref(), Some("algorithm 2"));
        assert_eq!(a.disks_status, Some((3, 3)));
        assert_eq!(a.status_glyphs.as_deref(), Some("[UUU]"));
        assert!(a.action.is_none());

        let by_name: std::collections::HashMap<_, _> =
            a.members.iter().map(|m| (m.kernel_name.as_str(), m)).collect();
        assert_eq!(by_name["loop11p1"].role, 0);
        assert_eq!(by_name["loop12p1"].role, 1);
        // The non-dense role: slot 2 was removed and the new disk got slot 3.
        assert_eq!(by_name["loop10p1"].role, 3);
        assert!(a.members.iter().all(|m| !m.failed && !m.spare));

        let bm = a.bitmap.as_ref().unwrap();
        assert_eq!(bm.pages_used, 0);
        assert_eq!(bm.pages_total, 1);
        assert_eq!(bm.chunk_kib, Some(65536));
    }

    #[test]
    fn parses_recovering_raid1_with_progress() {
        let s = parse_mdstat(RECOVERING_RAID1);
        assert_eq!(s.arrays.len(), 1);
        let a = &s.arrays[0];
        assert_eq!(a.disks_status, Some((2, 1)));
        assert_eq!(a.status_glyphs.as_deref(), Some("[U_]"));

        let act = a.action.as_ref().expect("recovery action expected");
        assert_eq!(act.kind, MdActionKind::Recovery);
        assert!((act.percent - 27.5).abs() < 0.001);
        assert_eq!(act.done_blocks, 288768);
        assert_eq!(act.total_blocks, 1048512);
        assert_eq!(act.speed_kib_s, 2048);
        assert!((act.finish_minutes - 2.3).abs() < 0.001);
    }

    #[test]
    fn parses_degraded_with_failed_member_and_no_action() {
        let s = parse_mdstat(DEGRADED_NO_REBUILD);
        let a = &s.arrays[0];
        assert_eq!(a.disks_status, Some((3, 2)));
        assert_eq!(a.status_glyphs.as_deref(), Some("[UU_]"));
        assert!(a.action.is_none());

        let failed: Vec<_> = a.members.iter().filter(|m| m.failed).collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].kernel_name, "sdc1");
        assert_eq!(failed[0].role, 2);
    }

    #[test]
    fn parses_raid0_without_status_glyphs() {
        let s = parse_mdstat(RAID0_NO_GLYPHS);
        let a = &s.arrays[0];
        assert_eq!(a.level.as_deref(), Some("raid0"));
        assert!(a.disks_status.is_none(), "raid0 has no [N/M]");
        assert!(a.status_glyphs.is_none(), "raid0 has no [UUU]");
        assert_eq!(a.blocks, Some(8388352));
    }

    #[test]
    fn parses_multiple_arrays_in_one_blob() {
        let s = parse_mdstat(TWO_ARRAYS);
        assert_eq!(s.arrays.len(), 2);
        assert_eq!(s.arrays[0].name, "md0");
        assert_eq!(s.arrays[0].level.as_deref(), Some("raid1"));
        assert_eq!(s.arrays[1].name, "md1");
        assert_eq!(s.arrays[1].level.as_deref(), Some("raid5"));
    }

    #[test]
    fn empty_input_yields_empty_snapshot() {
        let s = parse_mdstat("");
        assert!(s.personalities.is_empty());
        assert!(s.arrays.is_empty());
    }

    #[test]
    fn header_only_no_arrays() {
        let s = parse_mdstat("Personalities : [raid1]\nunused devices: <none>\n");
        assert_eq!(s.personalities, vec!["raid1"]);
        assert!(s.arrays.is_empty());
    }

    #[test]
    fn member_token_with_multiple_flags() {
        let m = parse_member_token("sda1[7](F)(W)").unwrap();
        assert_eq!(m.kernel_name, "sda1");
        assert_eq!(m.role, 7);
        assert!(m.failed);
        assert!(m.write_mostly);
        assert!(!m.spare);
    }

    #[test]
    fn member_token_spare() {
        let m = parse_member_token("nvme0n1p1[4](S)").unwrap();
        assert!(m.spare);
        assert!(!m.failed);
    }

    #[test]
    fn parse_action_handles_check() {
        let s = "[=====>...............]  check = 27.0% (1000/4000) finish=1.5min speed=512K/sec";
        let a = parse_action_line(s).unwrap();
        assert_eq!(a.kind, MdActionKind::Check);
        assert_eq!(a.done_blocks, 1000);
        assert_eq!(a.total_blocks, 4000);
        assert_eq!(a.speed_kib_s, 512);
    }

    #[test]
    fn glyphs_must_be_only_u_or_underscore() {
        // "[3/3]" should NOT be confused with the glyph bracket.
        assert_eq!(parse_glyphs("4185088 blocks ... [3/3] [UUU]"), Some("[UUU]".into()));
        assert_eq!(parse_glyphs("blocks [3/3]"), None);
    }
}
