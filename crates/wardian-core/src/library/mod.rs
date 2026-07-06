pub mod deployments;
pub mod frontmatter;
pub mod section;
pub mod links;
pub mod metadata;

pub use deployments::{
    collect_skill_sources, get_target_skills_dir, scan_deployments, DeploymentScan, SkillSource,
};
pub use frontmatter::{extract_description, parse_frontmatter};
pub use section::{resolve_entry_path, LibrarySectionId, DEPLOYED_SKILL_SOURCE_FILE};
pub use links::{create_directory_link, copy_dir_all, deploy_skill_dir, remove_existing_deployment};
pub use metadata::MetadataStore;
