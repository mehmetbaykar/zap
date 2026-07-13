//! Utility functions for working with skills.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::Path;

use ai::skills::{
    provider_parent_directory_for_skills_root, provider_rank, ParsedSkill, SkillProvider,
};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::Icon;
use warp_util::local_or_remote_path::LocalOrRemotePath;
use warpui::prelude::MouseStateHandle;
use warpui::{AppContext, Element, EventContext, SingletonEntity};

use super::{SkillDescriptor, SkillManager};
use crate::ai::blocklist::view_util::render_provider_icon_button;

/// Deduplicates skills by **name and owning directory**, keeping a single best representative per
/// skill name within each directory.
///
/// Priority rules (when there are multiple skills with the same name):
///
/// 1. **Lower provider rank wins**: follows the [`SKILL_PROVIDER_DEFINITIONS`] order (index 0 = highest priority),
///    e.g. `Agents > Zap > Claude > …`.
/// 2. **On equal rank, the shorter reference path wins**: used as a stable tiebreak.
///
/// This implementation covers three scenarios:
/// - `npx skills` symlinks a same-named skill into `~/.agents/skills/` / `~/.warp/skills/` / `~/.claude/skills/`
///   (same name, different provider) → keep the higher-priority provider.
/// - A same-named skill exists in multiple directories at once (e.g. repo root + subdir) → keep each, letting the caller handle them by path context.
/// - Same name, different content (different provider) → keep the higher-priority provider.
///
/// Each element of `skill_paths` is a `(dir_path, skill_file_path)` tuple where
/// `dir_path` is the directory that owns the skill and participates in the dedup key.
///
/// **P0-3 prompt cache fix**: the returned Vec is sorted in `(name, reference)` lexicographic order.
/// Reason: `HashMap::into_values()` iteration order is unstable, and this return value goes into the
/// skills section of the system prompt; any order drift would invalidate the entire prompt cache for all upstream
/// providers (Anthropic / OpenAI / DeepSeek). Same nature as the P0-3 MCP tools sorting.
/// It currently dedups by `(name, owning directory)`, so different directories can keep the same-named skill simultaneously.
/// reference remains the secondary key for stable sorting, ensuring the output order is reproducible.
#[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
pub(crate) fn unique_skills(
    skill_paths: &[(LocalOrRemotePath, LocalOrRemotePath)],
    skills_by_path: &HashMap<LocalOrRemotePath, ParsedSkill>,
) -> Vec<SkillDescriptor> {
    let mut name_map: HashMap<(String, LocalOrRemotePath), SkillDescriptor> = HashMap::new();

    for (dir_path, path) in skill_paths {
        let Some(skill) = skills_by_path.get(path) else {
            continue;
        };
        let descriptor = SkillDescriptor::from(skill.clone());
        match name_map.entry((descriptor.name.clone(), dir_path.clone())) {
            Entry::Vacant(e) => {
                e.insert(descriptor);
            }
            Entry::Occupied(mut e) => {
                let new_rank = provider_rank(descriptor.provider);
                let existing_rank = provider_rank(e.get().provider);
                if new_rank < existing_rank
                    || (new_rank == existing_rank
                        && skill_reference_key(&descriptor.reference).len()
                            < skill_reference_key(&e.get().reference).len())
                {
                    e.insert(descriptor);
                }
            }
        }
    }

    let mut out: Vec<SkillDescriptor> = name_map.into_values().collect();
    // P0-3 fix: sort by (name, reference literal) lexicographically so the system prompt is stable.
    out.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| skill_reference_key(&a.reference).cmp(&skill_reference_key(&b.reference)))
    });
    out
}

/// Generates a literalized key for a `SkillReference` for sorting.
/// Uses `display_path` for `Path` references to avoid cross-platform boundary issues; `BundledSkillId`
/// uses the id string directly; the two keys won't collide (a bundled id contains no path separator).
fn skill_reference_key(reference: &ai::skills::SkillReference) -> String {
    match reference {
        ai::skills::SkillReference::Path(p) => p.display_path(),
        ai::skills::SkillReference::BundledSkillId(id) => id.clone(),
    }
}

/// Lists all skills applicable to the current working directory.
///
/// **Design note**: the old `list_skills_if_changed` did incremental sends under the cloud protocol (comparing against the
/// `conversation.latest_skills()` sent last round, returning `None` when unchanged) to save uplink tokens —— the warp backend
/// maintained conversation state, so it sufficed to retain them after the first round. After the project moved off the cloud, BYOP goes through stateless
/// `/chat/completions` like OpenAI/Anthropic; the system prompt is fully re-rendered on the client every round, so the data must be delivered every round,
/// otherwise the skills section in the system prompt would disappear from the second round on.
/// It is therefore simplified to return everything every round.
pub fn list_skills(working_directory: Option<&Path>, app: &AppContext) -> Vec<SkillDescriptor> {
    let working_directory =
        working_directory.map(|dir| LocalOrRemotePath::Local(dir.to_path_buf()));
    SkillManager::as_ref(app).get_skills_for_working_directory(working_directory.as_ref(), app)
}

/// Renders an 'open skill' button for blocklist AI actions and the code diff view.
pub fn render_skill_button<F>(
    button_label: &str,
    button_handle: MouseStateHandle,
    appearance: &Appearance,
    skill_provider: SkillProvider,
    icon_override: Option<Icon>,
    on_click: F,
) -> Box<dyn Element>
where
    F: FnMut(&mut EventContext) + 'static,
{
    let theme = appearance.theme();
    let logo_fill = internal_colors::fg_overlay_6(theme);

    let icon = icon_override.unwrap_or_else(|| skill_provider.icon());

    let color = if icon_override.is_some() {
        logo_fill
    } else {
        skill_provider.icon_fill(logo_fill)
    };

    render_provider_icon_button(
        button_label,
        button_handle,
        appearance,
        icon,
        color,
        on_click,
    )
}

/// Returns a branded icon override for well-known skill names.
pub fn icon_override_for_skill_name(name: &str) -> Option<Icon> {
    match name {
        "stripe-projects-cli" => Some(Icon::StripeLogo),
        _ => None,
    }
}

pub fn skill_path_from_location(location: &LocalOrRemotePath) -> Option<LocalOrRemotePath> {
    let mut current = Some(location.clone());
    while let Some(candidate_skill_dir) = current {
        if candidate_skill_dir
            .parent()
            .and_then(|provider_dir| provider_parent_directory_for_skills_root(&provider_dir))
            .is_some()
        {
            return Some(candidate_skill_dir.join("SKILL.md"));
        }
        current = candidate_skill_dir.parent();
    }
    None
}

#[cfg(test)]
#[path = "skill_utils_tests.rs"]
mod tests;
