//! The pass that brings the index up to date with the files on disk.
//!
//! The pass runs only when somebody asks for it. Nothing else in Embornal
//! starts it, because an index that moves under a command would make the queue
//! change while an agent reads it.
//!
//! What makes the pass cheap is one comparison: a file whose bytes hash to
//! what the index already holds is not parsed at all, and everything below it
//! stays as it is. Parsing is cheap and writing a summary is not, so the whole
//! design is built to reach the second one as rarely as it can.

use crate::code::db::Database;
use crate::code::node::{ContentHash, NodeKind, PoolKey};
use crate::code::tree::{self, Built};
use crate::code::walk;
use crate::code::{parse, repo};
use crate::config::CodeConfig;
use crate::error::{Error, Result};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use ulid::Ulid;

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexReport {
    pub collection: String,
    /// The files that the walk reached.
    pub files_seen: usize,
    /// The files that the grammar read. On a repository that did not change,
    /// this is zero, and that is the whole point of the hashes.
    pub files_parsed: usize,
    /// The files that left the repository since the last pass.
    pub files_removed: usize,
    /// Every node of the collection after the pass.
    pub nodes: usize,
    /// The nodes with no summary in the pool.
    pub stale: usize,
    /// The files that the grammar could not read.
    pub parse_errors: usize,
}

/// Brings one collection up to date.
///
/// With `all`, every file is parsed again, whatever its hash says. Use it
/// after a change to a grammar or to a query, which moves what a file gives up
/// without moving the file.
pub fn index(
    db: &mut Database,
    root: &Path,
    collection: &str,
    config: &CodeConfig,
    all: bool,
) -> Result<IndexReport> {
    let collection_id = ensure_collection(db, collection, root)?;
    let root_id = ensure_root(db, collection_id)?;

    let found = walk::walk(root, config);
    let stored = stored_files(db, collection_id)?;

    // Every directory that the files of this pass need, from the top down, so
    // that a parent always exists before its child asks for it.
    let mut wanted_dirs: BTreeSet<String> = BTreeSet::new();
    for file in &found {
        wanted_dirs.extend(walk::ancestors(&file.rel_path));
    }
    let mut dir_ids = ensure_dirs(db, collection_id, root_id, &wanted_dirs)?;

    let mut report = IndexReport {
        collection: collection.to_string(),
        files_seen: found.len(),
        ..IndexReport::default()
    };

    // The hash of every file after the pass, which the directories above them
    // then hash in turn.
    let mut file_hashes: BTreeMap<String, ContentHash> = BTreeMap::new();

    for file in &found {
        let source = std::fs::read(&file.path).map_err(|source| Error::Io {
            path: file.path.clone(),
            source,
        })?;
        let hash = ContentHash::of_bytes(&source);

        if !all
            && let Some((_, stored_hash)) = stored.get(&file.rel_path)
            && *stored_hash == hash
        {
            // The bytes did not move, so nothing below this file did either.
            file_hashes.insert(file.rel_path.clone(), hash);
            continue;
        }

        let parsed = parse::parse(&source, file.language)?;
        if parsed.parse_errors {
            report.parse_errors += 1;
        }
        let nodes = tree::build_file(&file.rel_path, &source, &parsed);

        // The subtree goes and comes back. Its summaries do not: they live in
        // the pool, under a key that the same code builds again.
        if let Some((id, _)) = stored.get(&file.rel_path) {
            db.conn().execute("DELETE FROM nodes WHERE id = ?", [id])?;
        }
        let parent = parent_of(&file.rel_path, root_id, &dir_ids);
        write_subtree(
            db,
            collection_id,
            parent,
            &nodes,
            file.language.as_str(),
            parsed.parse_errors,
        )?;

        file_hashes.insert(file.rel_path.clone(), hash);
        report.files_parsed += 1;
    }

    // A file that left the repository takes its nodes with it.
    let seen: BTreeSet<&String> = found.iter().map(|file| &file.rel_path).collect();
    for (rel_path, (id, _)) in &stored {
        if !seen.contains(rel_path) {
            db.conn().execute("DELETE FROM nodes WHERE id = ?", [id])?;
            report.files_removed += 1;
        }
    }

    drop_empty_dirs(db, collection_id, &wanted_dirs, &mut dir_ids)?;
    rehash_dirs(db, collection_id, root_id, &wanted_dirs, &file_hashes)?;

    db.conn().execute(
        "UPDATE collections SET indexed_at = ? WHERE id = ?",
        params![Utc::now().to_rfc3339(), collection_id],
    )?;

    report.nodes = count(
        db,
        "SELECT COUNT(*) FROM nodes WHERE collection_id = ?",
        collection_id,
    )?;
    report.stale = count(
        db,
        "SELECT COUNT(*) FROM nodes n
         LEFT JOIN summaries s ON s.pool_key = n.pool_key
         WHERE n.collection_id = ? AND s.id IS NULL",
        collection_id,
    )?;
    Ok(report)
}

/// Finds the collection, or makes it.
fn ensure_collection(db: &Database, name: &str, root: &Path) -> Result<i64> {
    let existing: Option<i64> = db
        .conn()
        .query_row("SELECT id FROM collections WHERE name = ?", [name], |row| {
            row.get(0)
        })
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }

    db.conn().execute(
        "INSERT INTO collections(ulid, name, root, created_at) VALUES (?, ?, ?, ?)",
        params![
            Ulid::generate().to_string(),
            name,
            root.to_string_lossy(),
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(db.conn().last_insert_rowid())
}

/// Finds the node at the top of a collection, or makes it.
fn ensure_root(db: &Database, collection_id: i64) -> Result<i64> {
    let existing: Option<i64> = db
        .conn()
        .query_row(
            "SELECT id FROM nodes WHERE collection_id = ? AND kind = 'repo'",
            [collection_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }

    let hash = ContentHash::of_children([]);
    db.conn().execute(
        "INSERT INTO nodes(ulid, collection_id, parent_id, kind, name, qualified_name,
                           rel_path, depth, content_hash, pool_key)
         VALUES (?, ?, NULL, 'repo', '', '', '', 0, ?, ?)",
        params![
            Ulid::generate().to_string(),
            collection_id,
            hash.as_str(),
            PoolKey::new("", &hash).as_str()
        ],
    )?;
    Ok(db.conn().last_insert_rowid())
}

/// The file nodes that the index already holds, by their path.
fn stored_files(db: &Database, collection_id: i64) -> Result<BTreeMap<String, (i64, ContentHash)>> {
    let mut stmt = db.conn().prepare(
        "SELECT rel_path, id, content_hash FROM nodes
         WHERE collection_id = ? AND kind = 'file'",
    )?;
    let rows = stmt.query_map([collection_id], |row| {
        let rel_path: String = row.get(0)?;
        let id: i64 = row.get(1)?;
        let hash: String = row.get(2)?;
        Ok((rel_path, (id, ContentHash::from_stored(hash))))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Makes every directory that the pass needs, from the top down.
fn ensure_dirs(
    db: &Database,
    collection_id: i64,
    root_id: i64,
    wanted: &BTreeSet<String>,
) -> Result<HashMap<String, i64>> {
    let mut ids: HashMap<String, i64> = HashMap::new();
    let mut stmt = db
        .conn()
        .prepare("SELECT rel_path, id FROM nodes WHERE collection_id = ? AND kind = 'dir'")?;
    for row in stmt.query_map([collection_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })? {
        let (rel_path, id) = row?;
        ids.insert(rel_path, id);
    }

    // A BTreeSet gives the paths in order, so `src` always comes before
    // `src/memory` and a parent is there when its child needs one.
    for path in wanted {
        if ids.contains_key(path) {
            continue;
        }
        let parent = parent_of(path, root_id, &ids);
        let hash = ContentHash::of_children([]);
        db.conn().execute(
            "INSERT INTO nodes(ulid, collection_id, parent_id, kind, name, qualified_name,
                               rel_path, depth, content_hash, pool_key)
             VALUES (?, ?, ?, 'dir', ?, ?, ?, ?, ?, ?)",
            params![
                Ulid::generate().to_string(),
                collection_id,
                parent,
                path.rsplit('/').next().unwrap_or(path),
                path,
                path,
                depth_of(path) as i64,
                hash.as_str(),
                PoolKey::new(path, &hash).as_str(),
            ],
        )?;
        ids.insert(path.clone(), db.conn().last_insert_rowid());
    }
    Ok(ids)
}

/// Removes the directories that hold nothing any more.
fn drop_empty_dirs(
    db: &Database,
    collection_id: i64,
    wanted: &BTreeSet<String>,
    ids: &mut HashMap<String, i64>,
) -> Result<()> {
    let gone: Vec<String> = ids
        .keys()
        .filter(|path| !wanted.contains(*path))
        .cloned()
        .collect();
    for path in gone {
        if let Some(id) = ids.remove(&path) {
            db.conn().execute(
                "DELETE FROM nodes WHERE id = ? AND collection_id = ?",
                params![id, collection_id],
            )?;
        }
    }
    Ok(())
}

/// Hashes every directory and then the root, from the deepest up.
///
/// This is the one place where the tree is a Merkle tree. A directory holds no
/// bytes of its own, so what it holds can only reach it through its children.
fn rehash_dirs(
    db: &Database,
    collection_id: i64,
    root_id: i64,
    dirs: &BTreeSet<String>,
    files: &BTreeMap<String, ContentHash>,
) -> Result<()> {
    // The children of each directory, by name, so that the order is fixed.
    let mut children: BTreeMap<String, BTreeMap<String, ContentHash>> = BTreeMap::new();
    let mut top: BTreeMap<String, ContentHash> = BTreeMap::new();

    for (rel_path, hash) in files {
        match parent_path(rel_path) {
            Some(parent) => {
                children
                    .entry(parent)
                    .or_default()
                    .insert(rel_path.clone(), hash.clone());
            }
            None => {
                top.insert(rel_path.clone(), hash.clone());
            }
        }
    }

    // The deepest directory first, so that a directory is hashed only after
    // every directory below it already is.
    let mut ordered: Vec<&String> = dirs.iter().collect();
    ordered.sort_by_key(|path| std::cmp::Reverse(depth_of(path)));

    for path in ordered {
        let own = children.remove(path).unwrap_or_default();
        let hash = ContentHash::of_children(own.values());
        update_hash(db, collection_id, path, &hash)?;

        match parent_path(path) {
            Some(parent) => {
                children
                    .entry(parent)
                    .or_default()
                    .insert(path.clone(), hash);
            }
            None => {
                top.insert(path.clone(), hash);
            }
        }
    }

    let root_hash = ContentHash::of_children(top.values());
    db.conn().execute(
        "UPDATE nodes SET content_hash = ?, pool_key = ? WHERE id = ?",
        params![
            root_hash.as_str(),
            PoolKey::new("", &root_hash).as_str(),
            root_id
        ],
    )?;
    Ok(())
}

fn update_hash(
    db: &Database,
    collection_id: i64,
    rel_path: &str,
    hash: &ContentHash,
) -> Result<()> {
    db.conn().execute(
        "UPDATE nodes SET content_hash = ?, pool_key = ?
         WHERE collection_id = ? AND kind = 'dir' AND rel_path = ?",
        params![
            hash.as_str(),
            PoolKey::new(rel_path, hash).as_str(),
            collection_id,
            rel_path
        ],
    )?;
    Ok(())
}

/// Writes the nodes of one file.
fn write_subtree(
    db: &Database,
    collection_id: i64,
    parent: i64,
    nodes: &[Built],
    language: &str,
    parse_errors: bool,
) -> Result<()> {
    let base = depth_of_file(&nodes[0].qualified_name);
    let mut ids: Vec<i64> = Vec::with_capacity(nodes.len());

    for node in nodes {
        let parent_id = match node.parent {
            Some(at) => ids[at],
            None => parent,
        };
        let depth = base + depth_below(nodes, node);
        db.conn().execute(
            "INSERT INTO nodes(ulid, collection_id, parent_id, kind, name, qualified_name,
                               rel_path, language, start_line, end_line, depth,
                               content_hash, pool_key, parse_errors)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                Ulid::generate().to_string(),
                collection_id,
                parent_id,
                node.kind.as_str(),
                node.name,
                node.qualified_name,
                nodes[0].qualified_name,
                language,
                node.start_line,
                node.end_line,
                depth as i64,
                node.content_hash.as_str(),
                node.pool_key.as_str(),
                i64::from(parse_errors && node.kind == NodeKind::File),
            ],
        )?;
        ids.push(db.conn().last_insert_rowid());
    }
    Ok(())
}

/// How far a node sits below the file that holds it.
fn depth_below(nodes: &[Built], node: &Built) -> u32 {
    let mut depth = 0;
    let mut at = node.parent;
    while let Some(parent) = at {
        depth += 1;
        at = nodes[parent].parent;
    }
    depth
}

/// The row of the directory that holds `rel_path`, or the root.
fn parent_of(rel_path: &str, root_id: i64, dirs: &HashMap<String, i64>) -> i64 {
    parent_path(rel_path)
        .and_then(|parent| dirs.get(&parent).copied())
        .unwrap_or(root_id)
}

/// The directory above a path, or `None` when the path sits at the top.
fn parent_path(rel_path: &str) -> Option<String> {
    let at = rel_path.rfind('/')?;
    Some(rel_path[..at].to_string())
}

/// How many parts a path holds. The root is zero.
fn depth_of(rel_path: &str) -> u32 {
    rel_path.split('/').count() as u32
}

fn depth_of_file(rel_path: &str) -> u32 {
    depth_of(rel_path)
}

fn count(db: &Database, sql: &str, collection_id: i64) -> Result<usize> {
    let value: i64 = db
        .conn()
        .query_row(sql, [collection_id], |row| row.get(0))?;
    Ok(value as usize)
}

/// The collection that a directory answers to, made if it is absent.
pub fn collection_name(root: &Path, given: Option<&str>) -> String {
    given
        .map(str::to_string)
        .unwrap_or_else(|| repo::default_collection(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// A repository that no other test touches.
    struct Repo {
        root: std::path::PathBuf,
    }

    impl Repo {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("embornal-index-{name}"));
            std::fs::remove_dir_all(&root).ok();
            std::fs::create_dir_all(&root).unwrap();
            Self {
                root: std::fs::canonicalize(&root).unwrap(),
            }
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn remove(&self, rel: &str) {
            std::fs::remove_file(self.root.join(rel)).unwrap();
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn db() -> Database {
        Database::open_in_memory(&Config::default()).unwrap()
    }

    fn run(db: &mut Database, repo: &Repo) -> IndexReport {
        index(db, &repo.root, "test", &CodeConfig::default(), false).unwrap()
    }

    /// The nodes of the collection as `kind qualified_name`, in tree order.
    fn shape(db: &Database) -> Vec<String> {
        let mut stmt = db
            .conn()
            .prepare("SELECT kind, qualified_name FROM nodes ORDER BY depth, qualified_name")
            .unwrap();
        stmt.query_map([], |row| {
            Ok(format!(
                "{} {}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    }

    fn hash_of(db: &Database, rel_path: &str) -> String {
        db.conn()
            .query_row(
                "SELECT content_hash FROM nodes WHERE rel_path = ? AND kind IN ('dir','file')
                 ORDER BY kind LIMIT 1",
                [rel_path],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn root_hash(db: &Database) -> String {
        db.conn()
            .query_row(
                "SELECT content_hash FROM nodes WHERE kind = 'repo'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn a_first_pass_builds_the_whole_tree() {
        let repo = Repo::new("first");
        repo.write("src/memory/api.rs", "fn recall() {}\n");
        repo.write("src/main.rs", "fn main() {}\n");

        let mut db = db();
        let report = run(&mut db, &repo);

        assert_eq!(report.files_seen, 2);
        assert_eq!(report.files_parsed, 2);
        assert_eq!(
            shape(&db),
            vec![
                "repo ",
                "dir src",
                "file src/main.rs",
                "dir src/memory",
                "function src/main.rs::main",
                "file src/memory/api.rs",
                "function src/memory/api.rs::recall",
            ]
        );
        // Nothing is described yet, so every node is stale.
        assert_eq!(report.stale, report.nodes);
    }

    #[test]
    fn a_second_pass_over_an_unchanged_repository_parses_nothing() {
        // This is the whole point of hashing a file: parsing is cheap, and
        // what a re-parse would cost is the summaries below it.
        let repo = Repo::new("unchanged");
        repo.write("src/a.rs", "fn a() {}\n");
        repo.write("src/b.rs", "fn b() {}\n");

        let mut db = db();
        assert_eq!(run(&mut db, &repo).files_parsed, 2);

        let second = run(&mut db, &repo);
        assert_eq!(second.files_parsed, 0);
        assert_eq!(second.files_seen, 2);
        assert_eq!(second.files_removed, 0);
    }

    #[test]
    fn a_changed_file_is_the_only_one_that_is_read_again() {
        let repo = Repo::new("changed");
        repo.write("src/a.rs", "fn a() {}\n");
        repo.write("src/b.rs", "fn b() {}\n");

        let mut db = db();
        run(&mut db, &repo);
        let before = hash_of(&db, "src/b.rs");

        repo.write("src/a.rs", "fn a() { work() }\n");
        let report = run(&mut db, &repo);

        assert_eq!(report.files_parsed, 1);
        // The file that did not change kept its hash, and therefore its
        // summary in the pool.
        assert_eq!(hash_of(&db, "src/b.rs"), before);
    }

    #[test]
    fn a_change_reaches_every_directory_above_it_and_the_root() {
        let repo = Repo::new("merkle");
        repo.write("src/memory/api.rs", "fn recall() {}\n");
        repo.write("docs/build.rs", "fn build() {}\n");

        let mut db = db();
        run(&mut db, &repo);
        let (src, memory, docs, root) = (
            hash_of(&db, "src"),
            hash_of(&db, "src/memory"),
            hash_of(&db, "docs"),
            root_hash(&db),
        );

        repo.write("src/memory/api.rs", "fn recall() { work() }\n");
        run(&mut db, &repo);

        assert_ne!(hash_of(&db, "src/memory"), memory, "the directory above it");
        assert_ne!(hash_of(&db, "src"), src, "and the one above that");
        assert_ne!(root_hash(&db), root, "and the root");
        // A directory that holds nothing that changed does not move.
        assert_eq!(hash_of(&db, "docs"), docs);
    }

    #[test]
    fn a_file_that_leaves_takes_its_nodes_with_it() {
        let repo = Repo::new("removed");
        repo.write("src/a.rs", "fn a() {}\n");
        repo.write("src/b.rs", "fn b() {}\n");

        let mut db = db();
        run(&mut db, &repo);

        repo.remove("src/b.rs");
        let report = run(&mut db, &repo);

        assert_eq!(report.files_removed, 1);
        let names = shape(&db);
        assert!(
            !names.iter().any(|line| line.contains("src/b.rs")),
            "{names:?}"
        );
        assert!(
            names.iter().any(|line| line.contains("src/a.rs")),
            "{names:?}"
        );
    }

    #[test]
    fn a_directory_that_empties_goes_as_well() {
        let repo = Repo::new("emptied");
        repo.write("src/a.rs", "fn a() {}\n");
        repo.write("old/b.rs", "fn b() {}\n");

        let mut db = db();
        run(&mut db, &repo);
        assert!(shape(&db).iter().any(|line| line == "dir old"));

        repo.remove("old/b.rs");
        run(&mut db, &repo);
        assert!(!shape(&db).iter().any(|line| line == "dir old"));
    }

    #[test]
    fn a_file_that_the_grammar_cannot_read_is_one_node_and_is_counted() {
        let repo = Repo::new("broken");
        repo.write(
            "src/a.rs",
            "fn a() {\n<<<<<<< HEAD\n=======\n>>>>>>> b\n}\n",
        );

        let mut db = db();
        let report = run(&mut db, &repo);

        assert_eq!(report.parse_errors, 1);
        let flagged: i64 = db
            .conn()
            .query_row(
                "SELECT parse_errors FROM nodes WHERE rel_path = 'src/a.rs' AND kind = 'file'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(flagged, 1);
        // No child: the index does not claim a shape that it could not read.
        let children: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM nodes WHERE rel_path = 'src/a.rs' AND kind <> 'file'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(children, 0);
    }

    #[test]
    fn a_file_that_is_fixed_stops_being_flagged() {
        let repo = Repo::new("fixed");
        repo.write(
            "src/a.rs",
            "fn a() {\n<<<<<<< HEAD\n=======\n>>>>>>> b\n}\n",
        );
        let mut db = db();
        assert_eq!(run(&mut db, &repo).parse_errors, 1);

        repo.write("src/a.rs", "fn a() {}\n");
        let report = run(&mut db, &repo);
        assert_eq!(report.parse_errors, 0);
        assert!(shape(&db).iter().any(|line| line == "function src/a.rs::a"));
    }

    #[test]
    fn a_second_collection_over_the_same_code_starts_with_nothing_to_do() {
        // The second criterion of the design: a fork costs nothing, and no row
        // of `summaries` is copied to make that true.
        let repo = Repo::new("fork");
        repo.write("src/a.rs", "fn a() {}\n");

        let mut db = db();
        let first = index(&mut db, &repo.root, "main", &CodeConfig::default(), false).unwrap();
        assert!(first.stale > 0);

        // Describe every node of the first collection.
        let keys: Vec<String> = db
            .conn()
            .prepare("SELECT pool_key FROM nodes")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for key in &keys {
            db.conn()
                .execute(
                    "INSERT INTO summaries(pool_key, summary, description, author, written_at)
                     VALUES (?, 'A summary.', 'A description.', 'default', '2026-09-03T00:00:00Z')",
                    [key],
                )
                .unwrap();
        }
        let again = index(&mut db, &repo.root, "main", &CodeConfig::default(), false).unwrap();
        assert_eq!(again.stale, 0);

        let forked = index(&mut db, &repo.root, "fork", &CodeConfig::default(), false).unwrap();
        assert_eq!(forked.nodes, again.nodes);
        assert_eq!(forked.stale, 0, "a fork must not pay for what is described");
    }

    #[test]
    fn asking_for_all_reads_every_file_again() {
        let repo = Repo::new("all");
        repo.write("src/a.rs", "fn a() {}\n");

        let mut db = db();
        run(&mut db, &repo);
        assert_eq!(run(&mut db, &repo).files_parsed, 0);

        let forced = index(&mut db, &repo.root, "test", &CodeConfig::default(), true).unwrap();
        assert_eq!(forced.files_parsed, 1);
    }

    #[test]
    fn an_empty_repository_is_a_root_and_nothing_else() {
        let repo = Repo::new("bare");
        let mut db = db();
        let report = run(&mut db, &repo);

        assert_eq!(report.files_seen, 0);
        assert_eq!(shape(&db), vec!["repo "]);
    }

    #[test]
    fn a_file_at_the_top_hangs_from_the_root() {
        let repo = Repo::new("toplevel");
        repo.write("main.rs", "fn main() {}\n");

        let mut db = db();
        run(&mut db, &repo);
        assert_eq!(
            shape(&db),
            vec!["repo ", "file main.rs", "function main.rs::main"]
        );
    }
}
