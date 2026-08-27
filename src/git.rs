//! Git status for the sidebar's secondary line: counts of uncommitted changes
//! plus unpushed commits, computed for a zone's directory. Git runs as a
//! subprocess (no git library); the work is async so it never stalls the UI.

use gtk4::gio;
use std::ffi::OsStr;

/// A directory's git state, as rendered on the zone row's secondary line.
pub struct GitSummary {
    /// Files added (staged adds + untracked).
    pub added: u32,
    /// Files modified (anything changed that isn't a pure add/delete).
    pub modified: u32,
    /// Files deleted (staged or unstaged).
    pub deleted: u32,
    /// Lines inserted across all tracked changes (staged + unstaged).
    pub insertions: u64,
    /// Lines deleted across all tracked changes.
    pub deletions: u64,
    /// Commits ahead of the upstream branch (i.e. unpushed). 0 if no upstream.
    pub ahead: u32,
}

/// Parse `git status --porcelain=v2 --branch` into (added, modified, deleted,
/// ahead). Classification per entry uses X (staged) and Y (unstaged) codes:
/// a 'D' on either side is a deletion, an 'A' on the staged side is an add,
/// everything else (modify/rename/copy/conflict) counts as a modification.
/// Untracked entries (`?`) count as adds; ignored entries (`!`) are skipped.
pub fn parse_status(out: &str) -> (u32, u32, u32, u32) {
    let (mut added, mut modified, mut deleted, mut ahead) = (0u32, 0u32, 0u32, 0u32);
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("# branch.ab ") {
            // "+A -B": A is commits ahead (unpushed), B behind.
            if let Some(tok) = rest.split_whitespace().next()
                && let Some(n) = tok.strip_prefix('+')
            {
                ahead = n.parse().unwrap_or(0);
            }
            continue;
        }
        if let Some(b) = line.as_bytes().first() {
            match b {
                b'#' | b'!' => continue,
                b'?' => {
                    added += 1;
                    continue;
                }
                b'1' | b'2' | b'u' => {
                    // Second whitespace token is the two-char XY status field.
                    let xy = line.split_whitespace().nth(1).unwrap_or("");
                    let mut chars = xy.chars();
                    let x = chars.next().unwrap_or('.');
                    let y = chars.next().unwrap_or('.');
                    if x == 'D' || y == 'D' {
                        deleted += 1;
                    } else if x == 'A' {
                        added += 1;
                    } else {
                        modified += 1;
                    }
                }
                _ => {}
            }
        }
    }
    (added, modified, deleted, ahead)
}

/// Parse `git diff --numstat` into (insertions, deletions). Each line is
/// `<added>\t<deleted>\t<path>`; binary files report `-\t-` and are skipped.
pub fn parse_numstat(out: &str) -> (u64, u64) {
    let (mut ins, mut del) = (0u64, 0u64);
    for line in out.lines() {
        let mut cols = line.split('\t');
        let a = cols.next().unwrap_or("");
        let b = cols.next().unwrap_or("");
        if let (Ok(x), Ok(y)) = (a.parse::<u64>(), b.parse::<u64>()) {
            ins += x;
            del += y;
        }
    }
    (ins, del)
}

/// Render the summary as e.g. `+2 ~5 -1  +128 -34  ↑3`: a file-count group
/// (`+added ~modified -deleted`), a line-count group (`+ins -del`), and an
/// unpushed group (`↑ahead`), each omitted when empty and separated by two
/// spaces. A clean, fully-pushed repo renders as `✓`.
pub fn format_summary(s: &GitSummary) -> String {
    let mut groups: Vec<String> = Vec::new();

    let mut files: Vec<String> = Vec::new();
    if s.added > 0 {
        files.push(format!("+{}", s.added));
    }
    if s.modified > 0 {
        files.push(format!("~{}", s.modified));
    }
    if s.deleted > 0 {
        files.push(format!("-{}", s.deleted));
    }
    if !files.is_empty() {
        groups.push(files.join(" "));
    }

    if s.insertions > 0 || s.deletions > 0 {
        groups.push(format!("+{} -{}", s.insertions, s.deletions));
    }

    if s.ahead > 0 {
        groups.push(format!("↑{}", s.ahead));
    }

    if groups.is_empty() {
        "✓".to_string()
    } else {
        groups.join("  ")
    }
}

/// A colorizable piece of the rendered summary: display text — including any
/// leading separator spaces, so concatenating every segment's text reproduces
/// [`format_summary`] exactly — plus the CSS class the sidebar colors it with.
pub struct Segment {
    pub text: String,
    pub class: &'static str,
}

/// The summary as colorizable [`Segment`]s, mirroring [`format_summary`]'s
/// grouping and spacing but tagging each token with a CSS class so the sidebar
/// can color it independently. A clean repo yields a single `✓` (`git-clean`).
pub fn summary_segments(s: &GitSummary) -> Vec<Segment> {
    let mut groups: Vec<Vec<(String, &'static str)>> = Vec::new();

    let mut files = Vec::new();
    if s.added > 0 {
        files.push((format!("+{}", s.added), "git-added"));
    }
    if s.modified > 0 {
        files.push((format!("~{}", s.modified), "git-modified"));
    }
    if s.deleted > 0 {
        files.push((format!("-{}", s.deleted), "git-deleted"));
    }
    if !files.is_empty() {
        groups.push(files);
    }

    if s.insertions > 0 || s.deletions > 0 {
        groups.push(vec![
            (format!("+{}", s.insertions), "git-lines-added"),
            (format!("-{}", s.deletions), "git-lines-del"),
        ]);
    }

    if s.ahead > 0 {
        groups.push(vec![(format!("↑{}", s.ahead), "git-ahead")]);
    }

    if groups.is_empty() {
        return vec![Segment {
            text: "✓".to_string(),
            class: "git-clean",
        }];
    }

    // Flatten, re-inserting format_summary's separators (two spaces between
    // groups, one within a group) as leading whitespace on each token so the
    // concatenated segments round-trip to the same string.
    let mut segments = Vec::new();
    for (gi, group) in groups.into_iter().enumerate() {
        for (si, (token, class)) in group.into_iter().enumerate() {
            let sep = match (gi, si) {
                (0, 0) => "",
                (_, 0) => "  ",
                _ => " ",
            };
            segments.push(Segment {
                text: format!("{sep}{token}"),
                class,
            });
        }
    }
    segments
}

/// Run `git -C <cwd> <args...>` and return its stdout, or None if git could not
/// be spawned. stderr is silenced (e.g. the "not a git repository" message).
async fn git_output(cwd: &str, args: &[&str]) -> Option<String> {
    let mut argv: Vec<&OsStr> = vec![OsStr::new("git"), OsStr::new("-C"), OsStr::new(cwd)];
    argv.extend(args.iter().map(|a| OsStr::new(*a)));
    let flags = gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE;
    let proc = gio::Subprocess::newv(&argv, flags).ok()?;
    let (stdout, _) = proc.communicate_utf8_future(None).await.ok()?;
    Some(stdout.map(|s| s.to_string()).unwrap_or_default())
}

/// Compute the git summary for `cwd`, or None if it isn't a git repository
/// (or git is unavailable). A repo always emits `# branch.*` header lines under
/// `--branch`, so their presence is the repo test — no reliance on exit codes.
pub async fn run_summary(cwd: &str) -> Option<GitSummary> {
    let status_out = git_output(cwd, &["status", "--porcelain=v2", "--branch"]).await?;
    if !status_out.lines().any(|l| l.starts_with("# branch.")) {
        return None;
    }
    let (added, modified, deleted, ahead) = parse_status(&status_out);

    // Lines changed vs HEAD = unstaged diff + staged diff. Two calls rather than
    // `diff --numstat HEAD` so it also works in a repo with no commits yet.
    // Untracked files don't appear here — they only bump the file `added` count.
    let mut insertions = 0u64;
    let mut deletions = 0u64;
    for extra in [
        ["diff", "--numstat"].as_slice(),
        ["diff", "--numstat", "--cached"].as_slice(),
    ] {
        if let Some(out) = git_output(cwd, extra).await {
            let (i, d) = parse_numstat(&out);
            insertions += i;
            deletions += d;
        }
    }

    Some(GitSummary {
        added,
        modified,
        deleted,
        insertions,
        deletions,
        ahead,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "\
# branch.oid 0123456789abcdef
# branch.head main
# branch.upstream origin/main
# branch.ab +3 -0
1 .M N... 100644 100644 100644 aaa bbb file_modified.rs
1 A. N... 000000 100644 100644 ccc ddd file_added.rs
1 .D N... 100644 100644 000000 eee fff file_deleted.rs
2 R. N... 100644 100644 100644 ggg hhh R100 new.rs\told.rs
? untracked.txt
! ignored.log";

    #[test]
    fn status_classifies_entries_and_reads_ahead() {
        // added: A. entry + untracked = 2; modified: .M + rename = 2;
        // deleted: .D = 1; ahead: 3.
        let (added, modified, deleted, ahead) = parse_status(STATUS);
        assert_eq!((added, modified, deleted, ahead), (2, 2, 1, 3));
    }

    #[test]
    fn status_without_upstream_has_zero_ahead() {
        let out = "# branch.head main\n1 .M N... 100644 100644 100644 a b f.rs";
        let (_, modified, _, ahead) = parse_status(out);
        assert_eq!(modified, 1);
        assert_eq!(ahead, 0);
    }

    #[test]
    fn numstat_sums_and_skips_binaries() {
        let out = "10\t5\tfile_modified.rs\n20\t0\tfile_added.rs\n-\t-\tbinary.png";
        assert_eq!(parse_numstat(out), (30, 5));
    }

    #[test]
    fn format_combines_all_groups() {
        let s = GitSummary {
            added: 2,
            modified: 5,
            deleted: 1,
            insertions: 128,
            deletions: 34,
            ahead: 3,
        };
        assert_eq!(format_summary(&s), "+2 ~5 -1  +128 -34  ↑3");
    }

    #[test]
    fn format_omits_empty_groups() {
        let s = GitSummary {
            added: 0,
            modified: 1,
            deleted: 0,
            insertions: 4,
            deletions: 0,
            ahead: 0,
        };
        assert_eq!(format_summary(&s), "~1  +4 -0");
    }

    #[test]
    fn format_clean_repo_is_check() {
        let s = GitSummary {
            added: 0,
            modified: 0,
            deleted: 0,
            insertions: 0,
            deletions: 0,
            ahead: 0,
        };
        assert_eq!(format_summary(&s), "✓");
    }

    #[test]
    fn format_clean_but_ahead_shows_only_arrow() {
        let s = GitSummary {
            added: 0,
            modified: 0,
            deleted: 0,
            insertions: 0,
            deletions: 0,
            ahead: 2,
        };
        assert_eq!(format_summary(&s), "↑2");
    }

    /// The colored segments must reproduce format_summary's exact text (spacing
    /// included) when concatenated, so coloring never shifts the layout.
    fn joined(s: &GitSummary) -> String {
        summary_segments(s)
            .iter()
            .map(|seg| seg.text.as_str())
            .collect()
    }

    #[test]
    fn segments_round_trip_to_format() {
        for s in [
            GitSummary {
                added: 2,
                modified: 5,
                deleted: 1,
                insertions: 128,
                deletions: 34,
                ahead: 3,
            },
            GitSummary {
                added: 0,
                modified: 1,
                deleted: 0,
                insertions: 4,
                deletions: 0,
                ahead: 0,
            },
            GitSummary {
                added: 0,
                modified: 0,
                deleted: 0,
                insertions: 0,
                deletions: 0,
                ahead: 2,
            },
        ] {
            assert_eq!(joined(&s), format_summary(&s));
        }
    }

    #[test]
    fn segments_tag_each_token_with_a_class() {
        let s = GitSummary {
            added: 2,
            modified: 5,
            deleted: 1,
            insertions: 128,
            deletions: 34,
            ahead: 3,
        };
        let classes: Vec<&str> = summary_segments(&s).iter().map(|seg| seg.class).collect();
        assert_eq!(
            classes,
            [
                "git-added",
                "git-modified",
                "git-deleted",
                "git-lines-added",
                "git-lines-del",
                "git-ahead"
            ]
        );
    }

    #[test]
    fn segments_clean_repo_is_single_check() {
        let s = GitSummary {
            added: 0,
            modified: 0,
            deleted: 0,
            insertions: 0,
            deletions: 0,
            ahead: 0,
        };
        let segs = summary_segments(&s);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "✓");
        assert_eq!(segs[0].class, "git-clean");
    }
}
