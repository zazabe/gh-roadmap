use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "roadmap", about = "Sync local roadmap markdown with a work-tracking platform")]
pub struct Cli {
    /// Roadmap root directory (contains milestones/, issues/, roadmap.toml)
    #[arg(long, default_value = "roadmap", global = true)]
    pub roadmap_dir: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Validate local roadmap files (no network)
    Validate,
    /// Interactive setup: auth, discover project/repos, write roadmap.toml
    Setup {
        /// Overwrite existing roadmap.toml and skill files
        #[arg(long)]
        force: bool,
        /// Skip installing the agent skill
        #[arg(long)]
        no_skill: bool,
        /// Install the agent skill to this directory (default: .cursor/skills/roadmap)
        #[arg(long)]
        skill_dir: Option<PathBuf>,
        /// Only configure Notion report (requires existing roadmap.toml)
        #[arg(long)]
        notion_only: bool,
    },
    /// Show planned sync actions (dry-run)
    Plan {
        /// Use only local files and sync-map (no GitHub prefetch)
        #[arg(long)]
        offline: bool,
        /// Comma-separated platforms to include (github, notion). Default: all enabled.
        #[arg(long, value_name = "LIST")]
        platform: Option<String>,
    },
    /// Check GitHub authentication and token permissions
    Doctor,
    /// Apply planned actions to the remote platform
    Apply {
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
        /// Comma-separated platforms to include (github, notion). Default: all enabled.
        #[arg(long, value_name = "LIST")]
        platform: Option<String>,
    },
    /// Pull issue bodies + project field changes from GitHub into local markdown
    Pull {
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
        /// Overwrite local field values with remote values
        #[arg(long)]
        overwrite: bool,
        /// Import missing remote milestones/issues into local markdown
        #[arg(long)]
        import_missing: bool,
    },
    /// Show local + remote state for one issue (fields, sync drift)
    Show {
        /// GitHub issue number
        #[arg(long)]
        issue: u64,
        /// Local repo id from roadmap.toml (required if number is ambiguous)
        #[arg(long)]
        repo: Option<String>,
    },
}
