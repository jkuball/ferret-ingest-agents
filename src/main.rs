mod config;
mod ferret;
mod harnesses;
mod jsonl;
mod model;
mod render;
#[cfg(test)]
mod render_tests;
mod state;
mod templates;
mod views;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::Result;
use clap::{Parser, Subcommand};

use config::{Config, HarnessConfig, display};
use ferret::{Client, Outcome, Unauthorized};
use harnesses::Harness;
use render::Options;
use state::{FileState, State};
use templates::Templates;

#[derive(Parser)]
#[command(version, about = "Index coding agent sessions into ferret")]
struct Cli {
    /// Defaults to $XDG_CONFIG_HOME/ferret-ingest-agents/config.toml.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Send new and changed sessions to ferret (the default).
    Run {
        /// Report what would be sent without sending or saving anything.
        #[arg(long)]
        dry_run: bool,
        /// Only these session files, e.g. to try a few before indexing all.
        #[arg(long = "file", value_name = "FILE")]
        files: Vec<PathBuf>,
    },
    /// Print the documents a session file becomes, as JSON lines.
    Render {
        file: PathBuf,
        /// pi or claude-code; guessed from the path when left out.
        #[arg(long)]
        harness: Option<String>,
        /// A minijinja template replacing the built-in one.
        #[arg(long)]
        turn_template: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Run {
        dry_run: false,
        files: Vec::new(),
    }) {
        Command::Render {
            file,
            harness,
            turn_template,
        } => {
            let templates = Templates::load(turn_template.as_deref())?;
            let options = Options {
                host: config::default_host(),
                user: std::env::var("USER").unwrap_or_else(|_| "user".to_owned()),
            };
            let harness = match harness {
                Some(name) => Harness::from_name(&name)
                    .ok_or_else(|| anyhow::anyhow!("unknown harness {name:?}"))?,
                None if file.components().any(|c| c.as_os_str() == ".claude") => {
                    Harness::ClaudeCode
                }
                None => Harness::Pi,
            };
            let session = harness.read(&file)?;
            let source = config::default_source(&options.host, harness);
            for doc in render::documents(&session, &source, &display(&file), &options, &templates)?
            {
                println!("{}", serde_json::to_string(&doc)?);
            }
            Ok(())
        }
        Command::Run { dry_run, files } => {
            let config = Config::load(&cli.config.unwrap_or_else(Config::default_path))?;
            run(&config, dry_run, &files)
        }
    }
}

fn fingerprint(path: &Path) -> Result<(u64, i128)> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i128);
    Ok((meta.len(), mtime))
}

#[derive(Default)]
struct Counts {
    files: AtomicUsize,
    skipped: AtomicUsize,
    excluded: AtomicUsize,
    put: AtomicUsize,
    unchanged: AtomicUsize,
    deleted: AtomicUsize,
    failed: AtomicUsize,
}

fn run(config: &Config, dry_run: bool, only: &[PathBuf]) -> Result<()> {
    let options = Options {
        user: config.user.clone(),
        host: config.host.clone(),
    };
    let templates = Templates::load(config.turn_template.as_deref())?;
    let client = Client::new(&config.url, &config.key);
    let renderer = state::hash(
        format!(
            "{}\0{options:?}\0{:?}\0{}",
            env!("CARGO_PKG_VERSION"),
            config.include_exclude(),
            templates.sources
        )
        .as_bytes(),
    );
    let mut loaded = State::load(&config.state_file)?;
    if loaded.renderer.as_deref() != Some(renderer.as_str()) {
        // Every file is read and rendered again; document hashes still
        // keep what didn't change from being sent.
        for file in loaded.files.values_mut() {
            file.seen = None;
        }
    }
    let state = Mutex::new(loaded);
    let mut files: Vec<(&HarnessConfig, PathBuf)> = Vec::new();
    for harness in &config.harnesses {
        match harness.harness.discover(&harness.sessions_dir) {
            Ok(found) => files.extend(found.into_iter().map(|f| (harness, f))),
            // A harness that isn't installed on this machine has no sessions.
            Err(_) if !harness.sessions_dir.exists() => {}
            Err(error) => return Err(error),
        }
    }
    if !only.is_empty() {
        let wanted: Vec<PathBuf> = only
            .iter()
            .map(|f| std::fs::canonicalize(f).map_err(|e| anyhow::anyhow!("{}: {e}", f.display())))
            .collect::<Result<_>>()?;
        files.retain(|(_, f)| std::fs::canonicalize(f).is_ok_and(|f| wanted.contains(&f)));
        if files.len() < wanted.len() {
            anyhow::bail!("some --file paths are no session files of a configured harness");
        }
    }
    let queue = Mutex::new(files.iter());
    let counts = Counts::default();
    let stop = AtomicBool::new(false);
    let fatal: Mutex<Option<anyhow::Error>> = Mutex::new(None);

    std::thread::scope(|scope| {
        for _ in 0..config.concurrency {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let Some((harness, path)) = queue.lock().unwrap().next() else {
                        break;
                    };
                    let key = display(path);
                    let previous = state
                        .lock()
                        .unwrap()
                        .files
                        .get(&key)
                        .cloned()
                        .unwrap_or_default();
                    match sync_file(
                        config, harness, &client, &options, &templates, path, &previous, dry_run,
                        &counts,
                    ) {
                        Ok(Some(next)) if !dry_run => {
                            let mut state = state.lock().unwrap();
                            state.files.insert(key, next);
                            if counts.files.fetch_add(1, Ordering::Relaxed) % 50 == 49
                                && let Err(error) = state.save(&config.state_file)
                            {
                                eprintln!("saving the state: {error:#}");
                            }
                        }
                        Ok(_) => {}
                        Err(error) if error.is::<Unauthorized>() => {
                            stop.store(true, Ordering::Relaxed);
                            *fatal.lock().unwrap() = Some(error);
                        }
                        Err(error) => {
                            counts.failed.fetch_add(1, Ordering::Relaxed);
                            eprintln!("{key}: {error:#}");
                        }
                    }
                }
            });
        }
    });

    let c = |n: &AtomicUsize| n.load(Ordering::Relaxed);
    if !dry_run {
        let mut state = state.lock().unwrap();
        // Only a complete run over all files settles the renderer.
        if only.is_empty() && c(&counts.failed) == 0 && fatal.lock().unwrap().is_none() {
            state.renderer = Some(renderer);
        }
        state.save(&config.state_file)?;
    }
    eprintln!(
        "{} files: {} unchanged since the last run, {} excluded; documents: {} {}, {} unchanged, {} deleted; {} files failed",
        files.len(),
        c(&counts.skipped),
        c(&counts.excluded),
        c(&counts.put),
        if dry_run { "to send" } else { "sent" },
        c(&counts.unchanged),
        c(&counts.deleted),
        c(&counts.failed),
    );
    if let Some(error) = fatal.into_inner().unwrap() {
        return Err(error);
    }
    if c(&counts.failed) > 0 {
        anyhow::bail!("{} session files failed, see above", c(&counts.failed));
    }
    Ok(())
}

/// Sends one session file. Returns the file's new state, or None when it was
/// skipped because nothing changed.
#[allow(clippy::too_many_arguments)]
fn sync_file(
    config: &Config,
    harness: &HarnessConfig,
    client: &Client,
    options: &Options,
    templates: &Templates,
    path: &Path,
    previous: &FileState,
    dry_run: bool,
    counts: &Counts,
) -> Result<Option<FileState>> {
    let seen = match fingerprint(path) {
        Ok(seen) => seen,
        // Gone since it was listed, like a throwaway session.
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if previous.seen == Some(seen) {
        counts.skipped.fetch_add(1, Ordering::Relaxed);
        return Ok(None);
    }
    let session = harness.harness.read(path)?;

    let docs = if config.wants(&session.cwd) {
        render::documents(
            &session,
            &harness.source,
            &display(path),
            options,
            templates,
        )?
    } else {
        counts.excluded.fetch_add(1, Ordering::Relaxed);
        Vec::new()
    };

    let mut next = FileState {
        seen: None,
        docs: Default::default(),
    };
    for doc in &docs {
        let hash = state::hash(&serde_json::to_vec(doc)?);
        if previous.docs.get(&doc.external_id) == Some(&hash) {
            counts.unchanged.fetch_add(1, Ordering::Relaxed);
        } else if dry_run {
            counts.put.fetch_add(1, Ordering::Relaxed);
        } else {
            match client.put(doc)? {
                Outcome::Unchanged => counts.unchanged.fetch_add(1, Ordering::Relaxed),
                Outcome::Created | Outcome::Updated | Outcome::Excluded => {
                    counts.put.fetch_add(1, Ordering::Relaxed)
                }
            };
        }
        next.docs.insert(doc.external_id.clone(), hash);
    }

    // A session that was excluded, or a branch or turn that is gone. Deleted
    // session files are left alone, so ferret keeps them as an archive.
    for gone in previous
        .docs
        .keys()
        .filter(|id| !next.docs.contains_key(*id))
    {
        if !dry_run {
            client.delete(&harness.source, gone)?;
        }
        counts.deleted.fetch_add(1, Ordering::Relaxed);
    }

    // A file the harness is still writing is read again next time.
    if !session.torn {
        next.seen = Some(seen);
    }
    Ok(Some(next))
}
