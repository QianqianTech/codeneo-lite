//! Claude client configuration and launch helpers used by codeneo-lite.

mod accounts;
mod backup;
mod desktop_gateway;
mod history;
mod install;
mod ops;
mod paths;
mod presets;
mod profile;
mod settings;

pub use accounts::{
    clean_label, parse_ps_commands, remove_private_tree, scan_login_accounts, user_data_dir_arg,
    valid_extra_id, StoredAccount,
};
pub use backup::{backup_claude_cleanup, backup_tree, restore_tree, BackupError};
pub use desktop_gateway::{desktop_gateway_config, desktop_models};
pub use history::{
    config_backup_settings, create_settings_snapshot, list_config_backups, read_settings_snapshot,
    restore_config_backup, ConfigBackup, RestoreResult,
};
pub use install::{
    bundle_executable, cli_candidates, desktop_bundle_paths, effective_url_allowed,
    first_version_line, is_launchable_file, is_local_proxy_url, safe_exec_name, user_desktop_app,
    xml_plist_string, CLI_INSTALL_URL, DESKTOP_BUNDLE_ID, DESKTOP_INSTALL_URL,
};
pub use ops::{
    claude_code_script, claude_code_tun_script, code_shell_argv, code_shell_label,
    custom_model_launch, desktop_download_url, doctor_result, launch_plan, optional_help,
    project_conflicts, proxy_process_env, render_shown, shown_files, summarize_env,
    switch_confirm_text, switch_notice, updater_may_touch, CodeShell, EnvFacts, EnvOverview,
    LaunchKind, LaunchPlan, ShownFile, UpdaterSubject,
};
pub use paths::{choose_desktop_dir, current_os, desktop_layout, find_store_claude, HostOs, Layout};
pub use presets::{
    direct_provider, pinned_preset, preset_by_id, profile_from_preset, provider_presets, url_host,
    ProviderPreset,
};
pub use profile::{
    check_secrets, default_optional, merge_optional, normalize_base_url, normalize_desktop_models,
    reject_official_supplier_fields, AuthMode, OptionalVar, Profile, ProfileError,
};
pub use settings::{
    apply_profile, backup_file as backup_settings, conflict_keys, managed_key_names,
    prepare_profile, profile_env, read_env, read_settings_bytes, update_proxy_settings,
    verify_profile_settings, verify_profile_settings_for_tun, write_atomic, ProfileApply,
    SettingsError,
};
