//! The region picker's model: continents as aggregate rows over their locations, ordered by ping,
//! with a multiselect of location ids. Pure, so grouping, ordering, defaults and labels are tested
//! without an app; `lobby_ui` only draws what [`groups`] returns.
//!
//! - A location has a ping only when the Worker gave it a same-city beacon with a TCP port and a
//!   probe of it succeeded; otherwise it shows "no estimate" — never a neighbour's ping.
//! - A group's ping is the range of its locations' medians; groups sort by their lowest median,
//!   groups without one after every group with one, in the Worker's order. Locations inside a
//!   group sort the same way.
//! - The default selection is every location of the best group, applied once every probe has
//!   finished and only while the player has not touched the selection. With no estimate anywhere
//!   there is no best group, and nothing is ticked.

use std::collections::{BTreeMap, BTreeSet};

use super::contract::{JoinRequest, LocationsResponse};
use super::ping::PingSummary;

/// One location's ping as the picker shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Estimate {
    /// A probe is still running.
    Measuring,
    /// No same-city TCP beacon, or none of its samples succeeded.
    Unavailable,
    Measured(PingSummary),
}

impl Estimate {
    fn median_ms(&self) -> Option<f64> {
        match self {
            Estimate::Measured(summary) => Some(summary.median_ms),
            _ => None,
        }
    }
}

/// Estimates by location id. A location absent from the table is still [`Estimate::Measuring`].
pub(super) type PingTable = BTreeMap<String, Estimate>;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RowView {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) matches: u32,
    pub(super) estimate: Estimate,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct GroupView {
    pub(super) continent: String,
    pub(super) matches: u32,
    pub(super) rows: Vec<RowView>,
}

impl GroupView {
    /// `(lowest, highest)` median over the group's measured locations.
    pub(super) fn range_ms(&self) -> Option<(f64, f64)> {
        self.rows
            .iter()
            .filter_map(|row| row.estimate.median_ms())
            .fold(None, |range, ms| match range {
                None => Some((ms, ms)),
                Some((low, high)) => Some((f64::min(low, ms), f64::max(high, ms))),
            })
    }

    fn best_ms(&self) -> Option<f64> {
        self.range_ms().map(|(low, _)| low)
    }

    fn measuring(&self) -> bool {
        self.rows
            .iter()
            .any(|row| row.estimate == Estimate::Measuring)
    }

    pub(super) fn ids(&self) -> impl Iterator<Item = &str> {
        self.rows.iter().map(|row| row.id.as_str())
    }
}

/// Ascending by `key`, keyless entries last in their incoming order.
fn sort_by_ping<T>(items: &mut [T], key: impl Fn(&T) -> Option<f64>) {
    items.sort_by(|a, b| match (key(a), key(b)) {
        (Some(a), Some(b)) => a.total_cmp(&b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
}

/// The picker's rows, in display order.
pub(super) fn groups(locations: &LocationsResponse, pings: &PingTable) -> Vec<GroupView> {
    let mut groups: Vec<GroupView> = locations
        .continents
        .iter()
        .map(|continent| {
            let mut rows: Vec<RowView> = continent
                .locations
                .iter()
                .map(|location| RowView {
                    id: location.id.clone(),
                    label: if location.country.is_empty() || location.country == location.city {
                        location.city.clone()
                    } else {
                        format!("{}, {}", location.city, location.country)
                    },
                    matches: location.matches,
                    estimate: pings
                        .get(&location.id)
                        .copied()
                        .unwrap_or(Estimate::Measuring),
                })
                .collect();
            sort_by_ping(&mut rows, |row| row.estimate.median_ms());
            GroupView {
                continent: continent.continent.clone(),
                matches: continent.matches,
                rows,
            }
        })
        .collect();
    sort_by_ping(&mut groups, GroupView::best_ms);
    groups
}

/// Whether every probe has finished.
pub(super) fn settled(groups: &[GroupView]) -> bool {
    !groups.iter().any(GroupView::measuring)
}

/// Every location of the best group; empty when no location has an estimate.
pub(super) fn default_selection(groups: &[GroupView]) -> BTreeSet<String> {
    groups
        .first()
        .filter(|group| group.best_ms().is_some())
        .map(|group| group.ids().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// A group's tick box state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tick {
    All,
    Some,
    None,
}

impl Tick {
    pub(super) const fn glyph(self) -> &'static str {
        match self {
            Tick::All => "[x]",
            Tick::Some => "[-]",
            Tick::None => "[ ]",
        }
    }
}

pub(super) fn group_tick(group: &GroupView, selection: &BTreeSet<String>) -> Tick {
    let selected = group.ids().filter(|id| selection.contains(*id)).count();
    match selected {
        0 => Tick::None,
        n if n == group.rows.len() => Tick::All,
        _ => Tick::Some,
    }
}

/// A fully ticked group clears; any other group ticks all its locations.
pub(super) fn toggle_group(group: &GroupView, selection: &mut BTreeSet<String>) {
    if group_tick(group, selection) == Tick::All {
        for id in group.ids() {
            selection.remove(id);
        }
    } else {
        selection.extend(group.ids().map(str::to_owned));
    }
}

pub(super) fn toggle_location(id: &str, selection: &mut BTreeSet<String>) {
    if !selection.remove(id) {
        selection.insert(id.to_owned());
    }
}

/// The selected locations in display order, with a ping for each one that has an estimate.
pub(super) fn join_request(
    version: &str,
    groups: &[GroupView],
    selection: &BTreeSet<String>,
) -> JoinRequest {
    let selected = groups
        .iter()
        .flat_map(|group| &group.rows)
        .filter(|row| selection.contains(&row.id));
    let mut locations = Vec::new();
    let mut pings = BTreeMap::new();
    for row in selected {
        locations.push(row.id.clone());
        if let Estimate::Measured(summary) = row.estimate {
            pings.insert(row.id.clone(), summary.median_whole_ms());
        }
    }
    JoinRequest {
        version: version.to_owned(),
        locations,
        pings,
    }
}

pub(super) fn estimate_label(estimate: &Estimate) -> String {
    match estimate {
        Estimate::Measuring => "measuring…".into(),
        Estimate::Unavailable => "no estimate".into(),
        Estimate::Measured(summary) => format!("{:.0} ms", summary.median_ms),
    }
}

/// The spread beside a measured location, empty otherwise.
pub(super) fn spread_label(estimate: &Estimate) -> String {
    match estimate {
        Estimate::Measured(summary) => format!("spread {:.0}", summary.spread_ms),
        _ => String::new(),
    }
}

pub(super) fn group_ping_label(group: &GroupView) -> String {
    match group.range_ms() {
        Some((low, high)) if low.round() == high.round() => format!("{low:.0} ms"),
        Some((low, high)) => format!("{low:.0}–{high:.0} ms"),
        None if group.measuring() => "measuring…".into(),
        None => "no estimate".into(),
    }
}

pub(super) fn matches_label(matches: u32) -> String {
    match matches {
        1 => "1 match".into(),
        n => format!("{n} matches"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::contract::{Beacon, Continent, Location};
    use super::*;

    fn location(id: &str, matches: u32) -> Location {
        Location {
            id: id.into(),
            city: id.into(),
            country: String::new(),
            beacon: Some(Beacon {
                host: "203.0.113.1".into(),
                tcp_port: Some(1),
            }),
            matches,
        }
    }

    fn continent(name: &str, locations: Vec<Location>) -> Continent {
        Continent {
            continent: name.into(),
            matches: locations.iter().map(|l| l.matches).sum(),
            locations,
        }
    }

    fn measured(ms: f64) -> Estimate {
        Estimate::Measured(PingSummary {
            median_ms: ms,
            spread_ms: 4.0,
            samples: 5,
        })
    }

    /// Europe, Asia, Oceania as the Worker lists them.
    fn fixture() -> LocationsResponse {
        LocationsResponse {
            version: "0.0.0".into(),
            continents: vec![
                continent(
                    "Europe",
                    vec![location("Paris", 0), location("Frankfurt", 2)],
                ),
                continent(
                    "Asia",
                    vec![
                        location("Mumbai", 0),
                        location("Tokyo", 0),
                        location("Singapore", 1),
                    ],
                ),
                continent("Oceania", vec![location("Melbourne", 0)]),
            ],
        }
    }

    fn table(entries: &[(&str, Estimate)]) -> PingTable {
        entries
            .iter()
            .map(|(id, estimate)| ((*id).to_owned(), *estimate))
            .collect()
    }

    fn settled_pings() -> PingTable {
        table(&[
            ("Paris", measured(228.0)),
            ("Frankfurt", measured(210.0)),
            ("Mumbai", Estimate::Unavailable),
            ("Tokyo", measured(107.0)),
            ("Singapore", measured(45.0)),
            ("Melbourne", Estimate::Unavailable),
        ])
    }

    fn order(groups: &[GroupView]) -> Vec<(&str, Vec<&str>)> {
        groups
            .iter()
            .map(|group| (group.continent.as_str(), group.ids().collect()))
            .collect()
    }

    #[test]
    fn groups_sort_by_lowest_ping_and_unmeasured_go_last() {
        let groups = groups(&fixture(), &settled_pings());
        assert_eq!(
            order(&groups),
            vec![
                ("Asia", vec!["Singapore", "Tokyo", "Mumbai"]),
                ("Europe", vec!["Frankfurt", "Paris"]),
                ("Oceania", vec!["Melbourne"]),
            ]
        );
        assert_eq!(groups[0].range_ms(), Some((45.0, 107.0)));
        assert_eq!(groups[0].matches, 1);
        assert_eq!(groups[2].range_ms(), None);
    }

    #[test]
    fn labels_show_range_ping_and_no_estimate() {
        let groups = groups(&fixture(), &settled_pings());
        assert_eq!(group_ping_label(&groups[0]), "45–107 ms");
        assert_eq!(group_ping_label(&groups[2]), "no estimate");
        let mumbai = &groups[0].rows[2];
        assert_eq!(estimate_label(&mumbai.estimate), "no estimate");
        assert_eq!(spread_label(&mumbai.estimate), "");
        let singapore = &groups[0].rows[0];
        assert_eq!(estimate_label(&singapore.estimate), "45 ms");
        assert_eq!(spread_label(&singapore.estimate), "spread 4");
        assert_eq!(matches_label(1), "1 match");
        assert_eq!(matches_label(0), "0 matches");
    }

    #[test]
    fn single_measured_location_group_shows_one_value() {
        let pings = table(&[
            ("Paris", measured(228.0)),
            ("Frankfurt", Estimate::Unavailable),
        ]);
        let groups = groups(&fixture(), &pings);
        let europe = groups.iter().find(|g| g.continent == "Europe").unwrap();
        assert_eq!(group_ping_label(europe), "228 ms");
    }

    #[test]
    fn unprobed_locations_read_as_measuring_and_block_settling() {
        let pings = table(&[("Singapore", measured(45.0))]);
        let groups = groups(&fixture(), &pings);
        assert!(!settled(&groups));
        let oceania = groups.iter().find(|g| g.continent == "Oceania").unwrap();
        assert_eq!(group_ping_label(oceania), "measuring…");
        assert!(settled(&super::groups(&fixture(), &settled_pings())));
    }

    #[test]
    fn default_ticks_every_location_of_the_best_continent() {
        let groups = groups(&fixture(), &settled_pings());
        let selection = default_selection(&groups);
        assert_eq!(
            selection.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["Mumbai", "Singapore", "Tokyo"],
            "the best group's unmeasured location is ticked with it"
        );
    }

    #[test]
    fn no_estimate_anywhere_ticks_nothing() {
        let pings = fixture()
            .continents
            .iter()
            .flat_map(|c| &c.locations)
            .map(|l| (l.id.clone(), Estimate::Unavailable))
            .collect();
        let groups = groups(&fixture(), &pings);
        assert!(settled(&groups));
        assert!(default_selection(&groups).is_empty());
        assert_eq!(
            order(&groups)[0].0,
            "Europe",
            "unmeasured groups keep the Worker's order"
        );
    }

    #[test]
    fn group_toggle_selects_all_then_clears() {
        let groups = groups(&fixture(), &settled_pings());
        let europe = &groups[1];
        let mut selection = BTreeSet::new();
        toggle_location("Paris", &mut selection);
        assert_eq!(group_tick(europe, &selection), Tick::Some);
        toggle_group(europe, &mut selection);
        assert_eq!(group_tick(europe, &selection), Tick::All);
        toggle_group(europe, &mut selection);
        assert_eq!(group_tick(europe, &selection), Tick::None);
        assert!(selection.is_empty());
        toggle_location("Paris", &mut selection);
        toggle_location("Paris", &mut selection);
        assert!(selection.is_empty());
    }

    #[test]
    fn join_request_names_selected_in_display_order_and_omits_unmeasured_pings() {
        let groups = groups(&fixture(), &settled_pings());
        let selection: BTreeSet<String> = ["Mumbai", "Paris", "Singapore"]
            .into_iter()
            .map(String::from)
            .collect();
        let request = join_request("0.5.4", &groups, &selection);
        assert_eq!(request.version, "0.5.4");
        assert_eq!(request.locations, vec!["Singapore", "Mumbai", "Paris"]);
        assert_eq!(
            request.pings,
            BTreeMap::from([("Paris".to_owned(), 228), ("Singapore".to_owned(), 45)])
        );
    }

    #[test]
    fn labels_join_city_and_distinct_country() {
        let mut response = fixture();
        response.continents[0].locations[0].country = "France".into();
        let groups = groups(&response, &PingTable::new());
        let paris = groups
            .iter()
            .flat_map(|g| &g.rows)
            .find(|r| r.id == "Paris")
            .unwrap();
        assert_eq!(paris.label, "Paris, France");
    }
}
