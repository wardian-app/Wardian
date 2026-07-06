pub mod deployments;
pub mod frontmatter;
pub mod index;
pub mod section;
pub mod links;
pub mod metadata;
pub mod mutations;

pub use deployments::{
    collect_skill_sources, get_target_skills_dir, scan_deployments, DeploymentScan, SkillSource,
};
pub use frontmatter::{extract_description, parse_frontmatter};
pub use index::build_library_index;
pub use section::{resolve_entry_path, LibrarySectionId, DEPLOYED_SKILL_SOURCE_FILE};
pub use links::{create_directory_link, copy_dir_all, deploy_skill_dir, remove_existing_deployment};
pub use metadata::MetadataStore;
pub use mutations::{read_item, save_item, create_folder, update_metadata};
