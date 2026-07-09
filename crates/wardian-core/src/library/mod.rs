pub mod deployments;
pub mod frontmatter;
pub mod index;
pub mod section;
pub mod links;
pub mod metadata;
pub mod mutations;

pub use deployments::{
    collect_skill_sources, deploy_skill, get_target_skills_dir, remove_deployed_skill,
    scan_deployments, set_skill_deployments, DeploymentScan, SetDeploymentsOutcome, SkillSource,
};
pub use frontmatter::{extract_description, parse_frontmatter};
pub use index::build_library_index;
pub use section::{
    is_single_normal_component, resolve_entry_path, LibrarySectionId, DEPLOYED_SKILL_SOURCE_FILE,
};
pub use links::{create_directory_link, copy_dir_all, deploy_skill_dir, remove_existing_deployment};
pub use metadata::MetadataStore;
pub use mutations::{
    create_folder, delete_entry, read_item, remove_orphan_deployment, rename_entry, save_item,
    update_metadata, validate_entry_destination,
};
