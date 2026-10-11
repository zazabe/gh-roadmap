//! Push-only Notion milestone report (via `ntn` CLI).

mod apply;
mod client;
mod mapping;
mod plan;
pub mod setup;

pub use apply::apply_notion;
pub use client::NotionClient;
pub use plan::{build_notion_plan, print_notion_plan};
