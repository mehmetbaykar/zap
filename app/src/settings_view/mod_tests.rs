use pathfinder_geometry::vector::vec2f;
use settings_page::{
    Category, CategoryHeader, FilteredPageType, MatchData, PageTitle, PageType, SettingsWidget,
    categories_with_visible_content, search_terms_match,
};
use warpui::elements::Empty;
use warpui::platform::WindowStyle;
use warpui::{
    App, AppContext, Element, Entity, Presenter, TypedActionView, View, WindowInvalidation,
};

use super::*;
use crate::appearance::Appearance;

// Zap: upstream's `plan_header_presentation` tests were not restored; the helper was removed
// together with the Account page and the billing-and-usage module (see the note in `mod.rs`).

// ── MatchData behavior ──────────────────────────────────────────────────────

#[test]
fn match_data_uncounted_true_is_truthy() {
    assert!(MatchData::Uncounted(true).is_truthy());
}

#[test]
fn match_data_uncounted_false_is_not_truthy() {
    assert!(!MatchData::Uncounted(false).is_truthy());
}

#[test]
fn match_data_countable_nonzero_is_truthy() {
    assert!(MatchData::Countable(3).is_truthy());
    assert!(MatchData::Countable(1).is_truthy());
}

#[test]
fn match_data_countable_zero_is_not_truthy() {
    assert!(!MatchData::Countable(0).is_truthy());
}

// ── Display labels ─────────────────────────────────────────────────

#[test]
fn subpage_display_names_are_correct() {
    // Display labels are localized in Zap; pin the English bundle.
    crate::i18n::init(Some("en"));

    assert_eq!(SettingsSection::WarpAgent.to_string(), "Zap Agent");
    assert_eq!(SettingsSection::AgentProfiles.to_string(), "Profiles");
    assert_eq!(SettingsSection::AgentMCPServers.to_string(), "MCP servers");
    assert_eq!(SettingsSection::AgentProviders.to_string(), "Providers");
    assert_eq!(SettingsSection::Knowledge.to_string(), "Knowledge");
    assert_eq!(
        SettingsSection::ThirdPartyCLIAgents.to_string(),
        "Third party CLI agents"
    );
    assert_eq!(
        SettingsSection::CodeIndexing.to_string(),
        "Language servers"
    );
    assert_eq!(
        SettingsSection::EditorAndCodeReview.to_string(),
        "Editor and Code Review"
    );
}

// ── slug / from_slug ───────────────────────────────────────────────

/// Every `SettingsSection` variant that can be navigated to.
///
/// `all_sections_list_is_exhaustive` keeps this honest: adding a variant
/// breaks the exhaustive match there, which is the prompt to add it here.
const ALL_SECTIONS: &[SettingsSection] = &[
    SettingsSection::About,
    SettingsSection::Appearance,
    SettingsSection::Features,
    SettingsSection::Keybindings,
    SettingsSection::ZapDrive,
    SettingsSection::Scripting,
    SettingsSection::Warpify,
    SettingsSection::WarpAgent,
    SettingsSection::AgentProfiles,
    SettingsSection::AgentMCPServers,
    SettingsSection::AgentProviders,
    SettingsSection::Knowledge,
    SettingsSection::ThirdPartyCLIAgents,
    SettingsSection::Network,
    SettingsSection::Privacy,
    SettingsSection::CodeIndexing,
    SettingsSection::EditorAndCodeReview,
];

/// Internal backing-page keys. They are never nav targets, so they never reach
/// persistence and are exempt from the slug round-trip contract.
const INTERNAL_SECTIONS: &[SettingsSection] = &[SettingsSection::AI];

/// Sections whose user-facing Display label has deliberately diverged from the
/// slug it was seeded from, because the slug is a stored contract that the
/// rename must not follow.
const SECTIONS_WITH_RENAMED_DISPLAY_LABELS: &[SettingsSection] = &[SettingsSection::CodeIndexing];

#[test]
fn all_sections_list_is_exhaustive() {
    fn is_listed(section: SettingsSection) -> bool {
        let known = match section {
            SettingsSection::About
            | SettingsSection::Appearance
            | SettingsSection::Features
            | SettingsSection::Keybindings
            | SettingsSection::ZapDrive
            | SettingsSection::Scripting
            | SettingsSection::Warpify
            | SettingsSection::AI
            | SettingsSection::WarpAgent
            | SettingsSection::AgentProfiles
            | SettingsSection::AgentMCPServers
            | SettingsSection::AgentProviders
            | SettingsSection::Knowledge
            | SettingsSection::ThirdPartyCLIAgents
            | SettingsSection::Network
            | SettingsSection::Privacy
            | SettingsSection::CodeIndexing
            | SettingsSection::EditorAndCodeReview => section,
        };
        ALL_SECTIONS.contains(&known) || INTERNAL_SECTIONS.contains(&known)
    }

    for section in ALL_SECTIONS.iter().chain(INTERNAL_SECTIONS) {
        assert!(is_listed(*section), "{section:?} is missing from the list");
    }
}

#[test]
fn every_section_round_trips_through_its_slug() {
    for section in ALL_SECTIONS {
        assert_eq!(
            SettingsSection::from_slug(section.slug()),
            Some(*section),
            "{section:?} should round-trip through its slug"
        );
    }
}

#[test]
fn slugs_are_unique_across_sections() {
    let mut slugs: Vec<&str> = ALL_SECTIONS.iter().map(|section| section.slug()).collect();
    let total = slugs.len();
    slugs.sort_unstable();
    slugs.dedup();
    assert_eq!(slugs.len(), total, "two sections share a slug");
}

#[test]
fn slugs_were_seeded_from_the_display_labels_they_replaced() {
    // Slugs were seeded from the (English) Display strings that used to double
    // as the persistence key, so no data migration was needed. Display is now
    // free to diverge; when it does, list the section in
    // SECTIONS_WITH_RENAMED_DISPLAY_LABELS rather than moving the slug, which
    // is a stored contract.
    crate::i18n::init(Some("en"));

    for section in ALL_SECTIONS {
        if SECTIONS_WITH_RENAMED_DISPLAY_LABELS.contains(section) {
            continue;
        }
        assert_eq!(
            section.slug(),
            section.to_string(),
            "{section:?} slug diverged from the Display label it was seeded from"
        );
    }
}

#[test]
fn renamed_sections_keep_the_slug_they_were_seeded_with() {
    // Zap relabeled the page "Language servers" (it carries only the local LSP
    // widget), but persisted sessions and `surface.settings.open --page` still
    // speak the original slug.
    crate::i18n::init(Some("en"));

    assert_eq!(
        SettingsSection::CodeIndexing.to_string(),
        "Language servers"
    );
    assert_eq!(
        SettingsSection::CodeIndexing.slug(),
        "Indexing and projects"
    );
}

#[test]
fn from_slug_accepts_legacy_spellings() {
    // The legacy "Oz" name and the pre-rebrand "Warp" names must resolve to the
    // current sections so existing deep links, persisted sessions and external
    // callers keep working after the user-facing renames.
    assert_eq!(
        SettingsSection::from_slug("Oz"),
        Some(SettingsSection::WarpAgent)
    );
    assert_eq!(
        SettingsSection::from_slug("Warp Agent"),
        Some(SettingsSection::WarpAgent)
    );
    assert_eq!(
        SettingsSection::from_slug("WarpDrive"),
        Some(SettingsSection::ZapDrive)
    );
    assert_eq!(
        SettingsSection::from_slug("Warp Drive"),
        Some(SettingsSection::ZapDrive)
    );
    assert_eq!(
        SettingsSection::from_slug("AgentProfiles"),
        Some(SettingsSection::AgentProfiles)
    );
    assert_eq!(
        SettingsSection::from_slug("AgentMCPServers"),
        Some(SettingsSection::AgentMCPServers)
    );
    assert_eq!(
        SettingsSection::from_slug("AgentProviders"),
        Some(SettingsSection::AgentProviders)
    );
    assert_eq!(
        SettingsSection::from_slug("ThirdPartyCLIAgents"),
        Some(SettingsSection::ThirdPartyCLIAgents)
    );
    assert_eq!(
        SettingsSection::from_slug("CodeIndexing"),
        Some(SettingsSection::CodeIndexing)
    );
    assert_eq!(
        SettingsSection::from_slug("EditorAndCodeReview"),
        Some(SettingsSection::EditorAndCodeReview)
    );
    // Sessions saved before `slug` existed persisted the localized Display
    // label; zh-CN "Network" is spelled with escapes to keep the source ASCII.
    assert_eq!(
        SettingsSection::from_slug("\u{7F51}\u{7EDC}"),
        Some(SettingsSection::Network)
    );
}

#[test]
fn from_slug_maps_superseded_page_names_to_the_page_that_replaced_them() {
    // `AI`, `Code` and `MCP Servers` named pages that have since been split or
    // moved. Persisted sessions and warpctrl callers still use them, so they
    // resolve here, at the boundary, rather than existing as sections of their
    // own that every caller would have to remember to normalize.
    assert_eq!(
        SettingsSection::from_slug("AI"),
        Some(SettingsSection::WarpAgent)
    );
    // "Code" named Zap's combined page before it split in two; its content is
    // what is now Editor and Code Review.
    assert_eq!(
        SettingsSection::from_slug("Code"),
        Some(SettingsSection::EditorAndCodeReview)
    );
    assert_eq!(
        SettingsSection::from_slug("MCP Servers"),
        Some(SettingsSection::AgentMCPServers)
    );
}

#[test]
fn from_slug_rejects_unknown_input() {
    assert_eq!(SettingsSection::from_slug("Not a page"), None);
    assert_eq!(SettingsSection::from_slug(""), None);
}

// ── Collapsed umbrella nav-stop behavior ────────────────────────────────────
// Verify that arrow-key navigation lands on a collapsed umbrella as a single
// stop (and activates it by jumping to the first subpage, which auto-expands
// the umbrella) instead of silently skipping over it.

use nav::{SettingsNavItem, SettingsUmbrella};

/// The Agents umbrella's subpages, mirroring `SettingsSection::ai_subpages`,
/// which `SettingsView::new` declares the umbrella with. Duplicated here rather
/// than shared so these tests can assert fixed nav-stop indices against a
/// deliberately trimmed sidebar.
const AGENT_SUBPAGES: &[SettingsSection] = &[
    SettingsSection::WarpAgent,
    SettingsSection::AgentProfiles,
    SettingsSection::AgentProviders,
    SettingsSection::AgentMCPServers,
    SettingsSection::Knowledge,
    SettingsSection::ThirdPartyCLIAgents,
];

/// Builds the nav-items layout used by `SettingsView::new`, matching the real
/// sidebar ordering (minus the flag-gated Network and Scripting pages) so tests
/// exercise realistic nav orders. Zap's sidebar leads with its two umbrellas,
/// so they are adjacent at nav indices 0 and 1.
fn realistic_nav_items() -> Vec<SettingsNavItem> {
    vec![
        SettingsNavItem::Umbrella(SettingsUmbrella::new("Agents", AGENT_SUBPAGES.to_vec())),
        SettingsNavItem::Umbrella(SettingsUmbrella::new(
            "Code",
            vec![
                SettingsSection::CodeIndexing,
                SettingsSection::EditorAndCodeReview,
            ],
        )),
        SettingsNavItem::Page(SettingsSection::Appearance),
        SettingsNavItem::Page(SettingsSection::Features),
        SettingsNavItem::Page(SettingsSection::Keybindings),
        SettingsNavItem::Page(SettingsSection::Warpify),
        SettingsNavItem::Page(SettingsSection::Privacy),
        SettingsNavItem::Page(SettingsSection::About),
    ]
}

/// Mutably flips an umbrella's `expanded` flag at `nav_index`.
fn set_expanded(nav_items: &mut [SettingsNavItem], nav_index: usize, expanded: bool) {
    if let Some(SettingsNavItem::Umbrella(u)) = nav_items.get_mut(nav_index) {
        u.expanded = expanded;
    } else {
        panic!("nav_items[{nav_index}] is not an Umbrella");
    }
}

#[test]
fn collapsed_umbrella_is_a_single_nav_stop() {
    let nav_items = realistic_nav_items();
    // All umbrellas default to collapsed.
    let stops = build_nav_stops(&nav_items, |_| true);

    // Expect: <Agents umbrella>, <Code umbrella>, Appearance, Features,
    // Keybindings, Warpify, Privacy, About.
    assert_eq!(
        stops,
        vec![
            NavStop::CollapsedUmbrella {
                nav_index: 0,
                first_subpage: SettingsSection::WarpAgent,
                last_subpage: SettingsSection::ThirdPartyCLIAgents,
            },
            NavStop::CollapsedUmbrella {
                nav_index: 1,
                first_subpage: SettingsSection::CodeIndexing,
                last_subpage: SettingsSection::EditorAndCodeReview,
            },
            NavStop::Section(SettingsSection::Appearance),
            NavStop::Section(SettingsSection::Features),
            NavStop::Section(SettingsSection::Keybindings),
            NavStop::Section(SettingsSection::Warpify),
            NavStop::Section(SettingsSection::Privacy),
            NavStop::Section(SettingsSection::About),
        ]
    );
}

#[test]
fn expanded_umbrella_produces_section_stop_per_subpage() {
    let mut nav_items = realistic_nav_items();
    // Expand the Agents umbrella so each of its subpages becomes a nav stop.
    set_expanded(&mut nav_items, 0, true);

    let stops = build_nav_stops(&nav_items, |_| true);

    // Expect: WarpAgent, AgentProfiles, AgentProviders, AgentMCPServers,
    // Knowledge, ThirdPartyCLIAgents, <Code umbrella>, Appearance, Features,
    // Keybindings, Warpify, Privacy, About.
    let sections: Vec<_> = stops
        .iter()
        .map(|s| match s {
            NavStop::Section(section) => format!("{section:?}"),
            NavStop::CollapsedUmbrella { nav_index, .. } => format!("Umbrella@{nav_index}"),
        })
        .collect();
    assert_eq!(
        sections,
        vec![
            "WarpAgent",
            "AgentProfiles",
            "AgentProviders",
            "AgentMCPServers",
            "Knowledge",
            "ThirdPartyCLIAgents",
            "Umbrella@1",
            "Appearance",
            "Features",
            "Keybindings",
            "Warpify",
            "Privacy",
            "About",
        ]
    );
}

#[test]
fn collapsed_umbrella_with_filtered_subpages_uses_first_visible_subpage() {
    // When a search filter hides the first subpage, activating the collapsed
    // umbrella should land on the *next* visible subpage (still auto-expanding).
    let nav_items = realistic_nav_items();

    let stops = build_nav_stops(&nav_items, |section| {
        // Hide WarpAgent (first Agents subpage); keep the rest.
        section != SettingsSection::WarpAgent
    });

    let agents_stop = stops
        .iter()
        .find(|s| matches!(s, NavStop::CollapsedUmbrella { nav_index: 0, .. }))
        .expect("Agents umbrella should still be a collapsed stop");

    match agents_stop {
        NavStop::CollapsedUmbrella {
            first_subpage,
            last_subpage,
            ..
        } => {
            assert_eq!(
                *first_subpage,
                SettingsSection::AgentProfiles,
                "WarpAgent is hidden by the filter, so the first visible subpage is AgentProfiles"
            );
            assert_eq!(
                *last_subpage,
                SettingsSection::ThirdPartyCLIAgents,
                "last_subpage is unaffected by hiding WarpAgent and should remain the last visible subpage"
            );
        }
        _ => unreachable!(),
    }
}

#[test]
fn umbrella_with_no_visible_subpages_is_skipped_entirely() {
    let nav_items = realistic_nav_items();

    let stops = build_nav_stops(&nav_items, |section| !AGENT_SUBPAGES.contains(&section));

    // The Agents umbrella's subpages are all hidden, so the entire umbrella
    // should be absent from the nav order.
    assert!(
        stops
            .iter()
            .all(|s| !matches!(s, NavStop::CollapsedUmbrella { nav_index: 0, .. })),
        "Agents umbrella should not appear when none of its subpages are visible"
    );
    // The still-visible Code umbrella remains a stop.
    assert!(
        stops
            .iter()
            .any(|s| matches!(s, NavStop::CollapsedUmbrella { nav_index: 1, .. }))
    );
}

#[test]
fn filtered_out_top_level_page_is_skipped() {
    let nav_items = realistic_nav_items();

    let stops = build_nav_stops(&nav_items, |section| section != SettingsSection::About);

    assert!(
        !stops
            .iter()
            .any(|s| matches!(s, NavStop::Section(SettingsSection::About))),
        "About should be filtered out entirely"
    );
    // But other pages remain.
    assert!(
        stops
            .iter()
            .any(|s| matches!(s, NavStop::Section(SettingsSection::Appearance)))
    );
}

// ── current_stop_index ──────────────────────────────────────────────────────

#[test]
fn current_stop_index_matches_section_stop() {
    let nav_items = realistic_nav_items();
    let stops = build_nav_stops(&nav_items, |_| true);

    let idx = current_stop_index(&stops, &nav_items, SettingsSection::Features);
    assert_eq!(idx, Some(3));
}

#[test]
fn current_stop_index_maps_subpage_to_collapsed_umbrella() {
    // Edge case: the user manually collapsed the Agents umbrella while still
    // on one of its subpages. The collapsed umbrella should match as the
    // current stop so arrow-key cycling continues from the umbrella's position.
    let nav_items = realistic_nav_items();
    let stops = build_nav_stops(&nav_items, |_| true);

    let idx = current_stop_index(&stops, &nav_items, SettingsSection::Knowledge);
    assert_eq!(
        idx,
        Some(0),
        "Knowledge is under the collapsed Agents umbrella at nav_index 0"
    );
}

#[test]
fn current_stop_index_returns_none_when_section_is_not_present() {
    let nav_items = realistic_nav_items();
    // Filter out all Agents subpages (and therefore the umbrella) entirely.
    let stops = build_nav_stops(&nav_items, |section| !AGENT_SUBPAGES.contains(&section));

    // Knowledge isn't directly in stops, and no remaining collapsed umbrella
    // contains it, so current_stop_index should return None.
    assert_eq!(
        current_stop_index(&stops, &nav_items, SettingsSection::Knowledge),
        None
    );
}

// ── next_stop_index wrapping ────────────────────────────────────────────────

#[test]
fn next_stop_index_wraps_at_ends() {
    assert_eq!(next_stop_index(0, 3, CycleDirection::Up), 2);
    assert_eq!(next_stop_index(2, 3, CycleDirection::Down), 0);
    assert_eq!(next_stop_index(1, 3, CycleDirection::Up), 0);
    assert_eq!(next_stop_index(1, 3, CycleDirection::Down), 2);
}

#[test]
fn next_stop_index_handles_single_stop() {
    assert_eq!(next_stop_index(0, 1, CycleDirection::Up), 0);
    assert_eq!(next_stop_index(0, 1, CycleDirection::Down), 0);
}

// ── End-to-end cycling (no search) ──────────────────────────────────────────
// These tests simulate the sequence of nav-stop activations that would result
// from repeatedly pressing Down/Up, ensuring a collapsed umbrella is never
// skipped over.

/// Computes the section that would become active after applying the direction
/// once, starting from `current`. Mirrors the final target-resolution step in
/// `cycle_pages`.
fn simulate_cycle(
    nav_items: &[SettingsNavItem],
    stops: &[NavStop],
    current: SettingsSection,
    direction: CycleDirection,
) -> SettingsSection {
    let active = current_stop_index(stops, nav_items, current)
        .expect("current should exist in stops in these tests");
    let next = next_stop_index(active, stops.len(), direction);
    match stops[next] {
        NavStop::Section(section) => section,
        NavStop::CollapsedUmbrella {
            first_subpage,
            last_subpage,
            ..
        } => match direction {
            CycleDirection::Up => last_subpage,
            CycleDirection::Down => first_subpage,
        },
    }
}

#[test]
fn arrow_down_from_last_page_wraps_to_collapsed_agents_first_subpage() {
    let nav_items = realistic_nav_items();
    let stops = build_nav_stops(&nav_items, |_| true);

    // The Agents umbrella leads the sidebar, so pressing Down from About (the
    // last stop) wraps around, auto-expands Agents and selects WarpAgent, rather
    // than skipping over it to the Code umbrella.
    let next = simulate_cycle(
        &nav_items,
        &stops,
        SettingsSection::About,
        CycleDirection::Down,
    );
    assert_eq!(next, SettingsSection::WarpAgent);
}

#[test]
fn arrow_up_from_code_indexing_with_collapsed_agents_lands_on_last_subpage() {
    let mut nav_items = realistic_nav_items();
    // The user is on CodeIndexing, so the Code umbrella is expanded.
    set_expanded(&mut nav_items, 1, true);
    let stops = build_nav_stops(&nav_items, |_| true);

    // Pressing Up from CodeIndexing should land on the collapsed Agents
    // umbrella, which resolves to ThirdPartyCLIAgents (last visible subpage)
    // so the user continues moving in natural reading order rather than being
    // jumped back to the top of the umbrella.
    let next = simulate_cycle(
        &nav_items,
        &stops,
        SettingsSection::CodeIndexing,
        CycleDirection::Up,
    );
    assert_eq!(next, SettingsSection::ThirdPartyCLIAgents);
}

#[test]
fn arrow_up_into_collapsed_umbrella_respects_search_filter_for_last_subpage() {
    let mut nav_items = realistic_nav_items();
    // The user is on CodeIndexing, so the Code umbrella is expanded.
    set_expanded(&mut nav_items, 1, true);
    // Hide the last two Agents subpages; the last *visible* subpage of the
    // still-collapsed Agents umbrella should be AgentMCPServers.
    let is_visible = |section: SettingsSection| {
        !matches!(
            section,
            SettingsSection::Knowledge | SettingsSection::ThirdPartyCLIAgents
        )
    };
    let stops = build_nav_stops(&nav_items, is_visible);

    // From CodeIndexing, Up should land on the last *visible* Agents subpage
    // (AgentMCPServers), not on the filtered-out Knowledge/ThirdPartyCLIAgents
    // or on the first subpage WarpAgent.
    let next = simulate_cycle(
        &nav_items,
        &stops,
        SettingsSection::CodeIndexing,
        CycleDirection::Up,
    );
    assert_eq!(next, SettingsSection::AgentMCPServers);
}

#[test]
fn arrow_down_from_expanded_last_subpage_leaves_umbrella() {
    let mut nav_items = realistic_nav_items();
    set_expanded(&mut nav_items, 0, true); // expand Agents
    let stops = build_nav_stops(&nav_items, |_| true);

    // ThirdPartyCLIAgents is the last Agents subpage; Down should leave the
    // umbrella for the next item in the nav order, the collapsed Code umbrella,
    // landing on its first subpage.
    let next = simulate_cycle(
        &nav_items,
        &stops,
        SettingsSection::ThirdPartyCLIAgents,
        CycleDirection::Down,
    );
    assert_eq!(next, SettingsSection::CodeIndexing);
}

#[test]
fn arrow_down_across_adjacent_collapsed_umbrellas() {
    let nav_items = realistic_nav_items();
    // Both the Agents and Code umbrellas are collapsed.
    let stops = build_nav_stops(&nav_items, |_| true);

    // From About, Down should wrap to the first Agents subpage (Agents
    // umbrella auto-expands).
    let next_after_about = simulate_cycle(
        &nav_items,
        &stops,
        SettingsSection::About,
        CycleDirection::Down,
    );
    assert_eq!(next_after_about, SettingsSection::WarpAgent);

    // From the Agents umbrella stop (i.e. the user is "on" WarpAgent which
    // maps back to the collapsed umbrella), pressing Down again should land
    // on the Code umbrella's first subpage.
    let next_after_agents = simulate_cycle(
        &nav_items,
        &stops,
        SettingsSection::WarpAgent,
        CycleDirection::Down,
    );
    assert_eq!(next_after_agents, SettingsSection::CodeIndexing);
}

#[test]
fn arrow_down_collapsed_umbrella_respects_search_filter() {
    let nav_items = realistic_nav_items();
    // Search filter hides WarpAgent and AgentProfiles so the first visible
    // Agents subpage is AgentProviders.
    let is_visible = |section: SettingsSection| {
        !matches!(
            section,
            SettingsSection::WarpAgent | SettingsSection::AgentProfiles
        )
    };
    let stops = build_nav_stops(&nav_items, is_visible);

    // From About, Down should wrap to AgentProviders (first visible subpage of
    // the still-collapsed Agents umbrella), not to WarpAgent / AgentProfiles.
    let next = simulate_cycle(
        &nav_items,
        &stops,
        SettingsSection::About,
        CycleDirection::Down,
    );
    assert_eq!(next, SettingsSection::AgentProviders);
}

// ── PageType filter lifecycle across a rebuild (APP-4922) ────────────────────
// Rebuilding a page's PageType resets its widget filter to every widget, so an
// active query has to be reapplied for only matching widgets to render. No page
// rebuilds itself on navigation any more (each subpage owns its own view), but
// these tests still pin the underlying PageType::Uncategorized filter lifecycle
// and the real search_terms_match predicate that the invariant rests on.

/// Minimal View so PageType<V> can be instantiated in a unit test without the
/// full SettingsView/ViewContext a real settings page requires.
struct TestSettingsView;

impl Entity for TestSettingsView {
    type Event = ();
}

impl View for TestSettingsView {
    fn ui_name() -> &'static str {
        "TestSettingsView"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        Empty::new().finish()
    }
}

/// A SettingsWidget whose only test-relevant state is its search terms; render
/// is never invoked by the filter lifecycle under test.
struct StubWidget {
    terms: &'static str,
}

impl SettingsWidget for StubWidget {
    type View = TestSettingsView;

    fn search_terms(&self) -> &str {
        self.terms
    }

    fn render(&self, _: &Self::View, _: &Appearance, _: &AppContext) -> Box<dyn Element> {
        Empty::new().finish()
    }
}

/// A fresh Uncategorized page mirroring build_page -> new_uncategorized: every
/// widget index visible by default.
fn stub_widgets_page() -> PageType<TestSettingsView> {
    let widgets: Vec<Box<dyn SettingsWidget<View = TestSettingsView>>> = vec![
        Box::new(StubWidget {
            terms: "warp agent global ai toggle",
        }),
        Box::new(StubWidget {
            terms: "active ai autosuggestions prompt",
        }),
        Box::new(StubWidget {
            terms: "ai input model api key",
        }),
        Box::new(StubWidget {
            terms: "file search fuzzy opener",
        }),
        Box::new(StubWidget {
            terms: "voice input",
        }),
    ];
    PageType::new_uncategorized(widgets, None)
}

/// Number of widgets the page would render under its current filter.
fn visible_widget_count<V: View>(page: &PageType<V>) -> usize {
    let FilteredPageType::Uncategorized { widgets, .. } = page.get_filtered() else {
        panic!("expected Uncategorized page");
    };
    widgets.len()
}

#[test]
fn search_terms_match_direct_unit_checks() {
    // Empty query matches everything (mirrors PageType::update_filter's guard).
    assert!(search_terms_match("warp agent global ai toggle", ""));
    // All-words, case-insensitive, non-contiguous.
    assert!(search_terms_match(
        "active ai autosuggestions prompt",
        "autosuggestions"
    ));
    assert!(search_terms_match(
        "active ai autosuggestions prompt",
        "ACTIVE AI"
    ));
    assert!(search_terms_match(
        "file search fuzzy opener",
        "file search"
    ));
    // Every word must appear.
    assert!(!search_terms_match(
        "warp agent global ai toggle",
        "file search"
    ));
    assert!(!search_terms_match(
        "active ai autosuggestions prompt",
        "autosuggestions key"
    ));
}

#[test]
fn rebuild_resets_filter_to_all_widgets() {
    // Searching "file search" matches exactly one widget. A freshly built page
    // (mirroring build_page -> new_uncategorized) resets the filter to every
    // widget, so without reapplying update_filter the page would show all
    // widgets.
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let mut page = stub_widgets_page();
            let md = page.update_filter("file search", ctx);
            assert!(md.is_truthy());
            assert_eq!(visible_widget_count(&page), 1);

            let rebuilt = stub_widgets_page();
            assert_eq!(
                visible_widget_count(&rebuilt),
                5,
                "rebuild resets the filter to all widgets when update_filter isn't reapplied"
            );
        });
    });
}

#[test]
fn rebuild_with_reapply_keeps_only_matching_widgets() {
    // The fix: after a rebuild, reapply update_filter with the active query so
    // only matching widgets render on the restored subpage.
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let mut page = stub_widgets_page();
            page.update_filter("file search", ctx);
            assert_eq!(visible_widget_count(&page), 1);

            let mut rebuilt = stub_widgets_page();
            rebuilt.update_filter("file search", ctx);
            assert_eq!(
                visible_widget_count(&rebuilt),
                1,
                "reapplying the filter after a rebuild keeps only matching widgets visible"
            );
        });
    });
}

#[test]
fn reapply_handles_multi_word_and_case() {
    // A multi-word, case-insensitive query survives the rebuild + reapply cycle.
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let mut page = stub_widgets_page();
            page.update_filter("AI INPUT", ctx);
            assert_eq!(visible_widget_count(&page), 1);

            let mut rebuilt = stub_widgets_page();
            rebuilt.update_filter("AI INPUT", ctx);
            assert_eq!(visible_widget_count(&rebuilt), 1);
        });
    });
}

#[test]
fn empty_query_after_reapply_shows_all_widgets() {
    // When the search is cleared, the subpage shows all widgets again.
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let mut page = stub_widgets_page();
            page.update_filter("agent", ctx);
            assert_eq!(visible_widget_count(&page), 1);

            let mut rebuilt = stub_widgets_page();
            rebuilt.update_filter("", ctx);
            assert_eq!(
                visible_widget_count(&rebuilt),
                5,
                "an empty query restores every widget on the subpage"
            );
        });
    });
}

struct NeverRendersWidget {
    terms: &'static str,
}

impl SettingsWidget for NeverRendersWidget {
    type View = TestSettingsView;

    fn search_terms(&self) -> &str {
        self.terms
    }

    fn should_render(&self, _: &AppContext) -> bool {
        false
    }

    fn render(&self, _: &Self::View, _: &Appearance, _: &AppContext) -> Box<dyn Element> {
        Empty::new().finish()
    }
}

#[test]
fn category_whose_sole_widget_cannot_render_has_no_visible_content_before_any_filter_pass() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let children: Vec<Box<dyn SettingsWidget<View = TestSettingsView>>> =
                vec![Box::new(NeverRendersWidget {
                    terms: "cloud handoff",
                })];
            let page =
                PageType::new_categorized(vec![Category::new("Cloud Handoff", children)], None);

            let FilteredPageType::Categorized { categories, .. } = page.get_filtered() else {
                panic!("expected Categorized page");
            };
            assert_eq!(
                categories.len(),
                1,
                "the untouched filter includes every widget index, so the category is still present here"
            );
            assert!(
                categories_with_visible_content(categories, ctx).is_empty(),
                "the category's sole widget can't render right now, so it has nothing visible to show"
            );
        });
    });
}

#[test]
fn category_whose_sole_widget_cannot_render_has_no_visible_content_after_an_empty_query() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let children: Vec<Box<dyn SettingsWidget<View = TestSettingsView>>> =
                vec![Box::new(NeverRendersWidget {
                    terms: "cloud handoff",
                })];
            let mut page =
                PageType::new_categorized(vec![Category::new("Cloud Handoff", children)], None);
            page.update_filter("", ctx);

            let FilteredPageType::Categorized { categories, .. } = page.get_filtered() else {
                panic!("expected Categorized page");
            };
            assert!(
                categories.is_empty(),
                "an empty-query filter pass already drops a category with no should_render widgets"
            );
        });
    });
}

/// A no-observable-output trailing-element closure, for testing attachment and visibility only.
fn stub_trailing_element(_: &TestSettingsView, _: &Appearance, _: &AppContext) -> Box<dyn Element> {
    Empty::new().finish()
}

/// An Uncategorized page with one widget plus a title trailing element.
fn uncategorized_page_with_title_trailing_element() -> PageType<TestSettingsView> {
    let widgets: Vec<Box<dyn SettingsWidget<View = TestSettingsView>>> =
        vec![Box::new(StubWidget {
            terms: "unrelated child setting",
        })];
    PageType::new_uncategorized(
        widgets,
        Some(PageTitle::new("Page").with_trailing_element(stub_trailing_element)),
    )
}

#[test]
fn title_trailing_element_is_present_regardless_of_widget_filter() {
    // The title trailing element takes no part in search: it must be present whether or not any
    // body widget matches, and must never affect MatchData.
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let mut page = uncategorized_page_with_title_trailing_element();
            let match_data = page.update_filter("totally unrelated query", ctx);
            assert!(!match_data.is_truthy());

            let FilteredPageType::Uncategorized { widgets, title, .. } = page.get_filtered() else {
                panic!("expected Uncategorized page");
            };
            assert!(widgets.is_empty());
            assert!(title.is_some_and(|t| t.trailing_element.is_some()));
        });
    });
}

/// A Categorized page with one category holding two child widgets and a trailing element.
fn categorized_page_with_trailing() -> PageType<TestSettingsView> {
    let children: Vec<Box<dyn SettingsWidget<View = TestSettingsView>>> = vec![
        Box::new(StubWidget {
            terms: "child one settings",
        }),
        Box::new(StubWidget {
            terms: "child two settings",
        }),
    ];
    let category = Category::with_header(
        CategoryHeader::new("Master").with_trailing_element(stub_trailing_element),
        children,
    );
    PageType::new_categorized(vec![category], None)
}

/// The number of widgets and whether the trailing element is present for the sole category of a
/// `categorized_page_with_trailing`-shaped page.
fn categorized_widget_and_trailing_state<V: View>(page: &PageType<V>) -> Vec<(usize, bool)> {
    let FilteredPageType::Categorized { categories, .. } = page.get_filtered() else {
        panic!("expected Categorized page");
    };
    categories
        .into_iter()
        .map(|c| (c.widgets.len(), c.trailing_element.is_some()))
        .collect()
}

#[test]
fn category_trailing_element_renders_alongside_a_matching_child() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let mut page = categorized_page_with_trailing();
            let match_data = page.update_filter("child one", ctx);
            assert!(match_data.is_truthy());
            assert_eq!(
                categorized_widget_and_trailing_state(&page),
                vec![(1, true)]
            );
        });
    });
}

#[test]
fn category_and_its_trailing_element_are_dropped_when_no_child_matches() {
    // The trailing element takes no part in search, so visibility is decided purely by the
    // children: a query can't resurface the category through the accessory.
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let mut page = categorized_page_with_trailing();
            let match_data = page.update_filter("totally unrelated query", ctx);
            assert!(!match_data.is_truthy());
            assert_eq!(categorized_widget_and_trailing_state(&page), vec![]);
        });
    });
}

#[test]
fn category_with_trailing_element_shows_everything_on_empty_query() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            let mut page = categorized_page_with_trailing();
            let match_data = page.update_filter("", ctx);
            assert!(match_data.is_truthy());
            assert_eq!(
                categorized_widget_and_trailing_state(&page),
                vec![(2, true)]
            );
        });
    });
}

/// Renders a `categorized_page_with_trailing` page, whose category has no subtitle (a
/// `render_sub_header` header, not `render_sub_header_with_description`).
struct CategoryHeaderTrailingElementTestView;

impl Entity for CategoryHeaderTrailingElementTestView {
    type Event = ();
}

impl View for CategoryHeaderTrailingElementTestView {
    fn ui_name() -> &'static str {
        "CategoryHeaderTrailingElementTestView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        categorized_page_with_trailing().render(&TestSettingsView, app)
    }
}

impl TypedActionView for CategoryHeaderTrailingElementTestView {
    type Action = ();
}

/// Regression test: a category header with a trailing element and no subtitle used to panic flex
/// layout (see `render_header_with_trailing_element`'s `Shrinkable` fix).
#[test]
fn category_header_with_trailing_element_and_no_subtitle_does_not_panic_flex_layout() {
    App::test((), |mut app| async move {
        let app = &mut app;
        app.add_singleton_model(|_| Appearance::mock());

        let (window_id, _view) = app.add_window(WindowStyle::NotStealFocus, |_| {
            CategoryHeaderTrailingElementTestView
        });
        let root_view_id = app
            .root_view_id(window_id)
            .expect("window should have a root view");

        let mut presenter = Presenter::new(window_id);
        let invalidation = WindowInvalidation {
            updated: [root_view_id].into_iter().collect(),
            ..Default::default()
        };

        app.update(move |ctx| {
            presenter.invalidate(invalidation, ctx);
            // Panicked here before the fix.
            presenter.build_scene(vec2f(800., 600.), 1., None, ctx);
        });
    });
}
