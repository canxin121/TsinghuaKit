use super::*;

#[test]
fn backend_repair_sep19_classroom_reference_whitespace_is_not_three_metadata_columns() {
    let headers = (0..7)
        .map(|i| format!("<td colspan=6>星期{}(09-{:02})</td>", i + 1, 14 + i))
        .collect::<String>();
    let slots = (0..42)
        .map(|i| {
            if i == 0 {
                "\n<td class='onteaching colBound'></td>"
            } else {
                "\n<td></td>"
            }
        })
        .collect::<String>();
    // Cheerio children[1] is the first cell, not the second element. The
    // preceding/newline nodes also explain reference children.slice(3).
    let html = format!(
        "<select id=weeknumber><option value=3>3</option></select><table><tr>{headers}</tr></table><div id=scrollContent><table><tbody><tr>\n<td>\n<span></span>Fixture 101</td>{slots}\n</tr></tbody></table></div>"
    );
    let result = parse_weekly_state(&html, 3).unwrap();
    assert_eq!(result.classroom_states[0].name, "Fixture 101");
    assert_eq!(result.classroom_states[0].status.len(), 42);
    assert_eq!(
        result.classroom_states[0].status[0],
        ClassroomSlotStatus::Occupied
    );
    assert_eq!(
        result.classroom_states[0].status[41],
        ClassroomSlotStatus::Free
    );
}
const LINK: &str = "/http/fixturemap/pk.classroomctrl.do?m=qyClassroomState&amp;classroom=Fixture&amp;weeknumber=3";

fn sep19_weekly_row(cells: &str) -> String {
    let headers = (0..7)
        .map(|i| format!("<td colspan=6>星期{}(09-{:02})</td>", i + 1, 14 + i))
        .collect::<String>();
    format!(
        "<select id=weeknumber><option value=3>3</option></select><table><tr>{headers}</tr></table><div id=scrollContent><table><tr>{cells}</tr></table></div>"
    )
}

#[test]
fn backend_repair_sep19_classroom_both_explicit_layouts_keep_unknown_and_all_slots() {
    let slots = format!(
        "<td class='unknown colBound'></td>{}",
        "<td></td>".repeat(41)
    );
    for prefix in [
        "<td>Fixture 101</td>",
        "<td>1</td><td>Fixture 101</td><td>meta</td>",
    ] {
        let parsed = parse_weekly_state(&sep19_weekly_row(&format!("{prefix}{slots}")), 3).unwrap();
        let row = &parsed.classroom_states[0];
        assert_eq!(row.name, "Fixture 101");
        assert_eq!(row.status.len(), 42);
        assert!(matches!(
            &row.status[0],
            ClassroomSlotStatus::Unknown { .. }
        ));
        assert_eq!(row.status[41], ClassroomSlotStatus::Free);
    }
}

#[test]
fn backend_repair_sep19_classroom_missing_extra_or_merged_slots_are_not_free_rooms() {
    for slots in [0, 40, 41, 43, 45] {
        let html = sep19_weekly_row(&format!(
            "<td>Fixture 101</td>{}",
            "<td></td>".repeat(slots)
        ));
        assert!(parse_weekly_state(&html, 3).is_err());
    }
    for cell in ["<td colspan=2></td>", "<td rowspan=2></td>", "<th></th>"] {
        let html = sep19_weekly_row(&format!(
            "<td>Fixture 101</td>{cell}{}",
            "<td></td>".repeat(41)
        ));
        assert!(parse_weekly_state(&html, 3).is_err());
    }
    assert!(
        parse_weekly_state(
            &sep19_weekly_row(&format!("<td> </td>{}", "<td></td>".repeat(42))),
            3
        )
        .is_err()
    );
}
#[test]
fn backend_repair_lastmile_classroom_accepts_optional_html_end_tags_like_reference() {
    for body in [
        format!(
            "<html><body><table><tr><td class=w30><a href='{LINK}'>Fixture Building</a><td>Other</table>"
        ),
        format!("<div class=w30><p>Buildings<p><a href='{LINK}'>Fixture Building</a></div>"),
    ] {
        let rows = parse_building_list(&body).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].search_name, "Fixture");
        assert_eq!(rows[0].week_number, 3);
    }
}
#[test]
fn backend_repair_lastmile_classroom_ignores_non_building_sidebar_links_not_valid_candidates() {
    let body = format!(
        "<html><div class='w30'><a href='/http/fixturemap/help'>帮助</a><a href='{LINK}'>Fixture Building</a></div></html>"
    );
    assert_eq!(parse_building_list(&body).unwrap().len(), 1);
    assert!(
        parse_building_list("<div class=w30><a href='/http/fixturemap/help'>帮助</a></div>")
            .is_err()
    );
}
#[test]
fn backend_repair_lastmile_classroom_invalid_actual_building_is_not_silently_skipped() {
    let bad = LINK.replace("weeknumber=3", "weeknumber=0");
    let body = format!("<div class=w30><a href='{bad}'>Bad</a><a href='{LINK}'>Fixture</a></div>");
    assert!(parse_building_list(&body).is_err());
}
#[test]
fn backend_repair_lastmile_classroom_results_match_real_reference_cheerio() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("reference_lastmile_fixtures.json")).unwrap();
    for sample in fixtures["classrooms"].as_array().unwrap() {
        let parsed = parse_building_list(sample["html"].as_str().unwrap()).unwrap();
        let expected = sample["expected"].as_array().unwrap();
        assert_eq!(parsed.len(), expected.len());
        for (row, want) in parsed.iter().zip(expected) {
            assert_eq!(row.name, want["name"]);
            assert_eq!(row.search_name, want["searchName"]);
            assert_eq!(
                u64::from(row.week_number),
                want["weekNumber"].as_u64().unwrap()
            );
        }
    }
}
#[test]
fn backend_repair_lastmile_classroom_weekly_html5_table_keeps_all_42_slots() {
    let headers = (0..7)
        .map(|i| format!("<td colspan=6>星期{}(09-{:02})", i + 1, 14 + i))
        .collect::<String>();
    let slots = (0..42)
        .map(|i| match i {
            0 => "<td class=onteaching>",
            1 => "<td class=onexam>",
            2 => "<td class=onborrowed>",
            3 => "<td class=ondisabled>",
            4 => "<td class=unexpected>",
            _ => "<td>",
        })
        .collect::<String>();
    let html = format!(
        "<html><select id=weeknumber><option value=3>3<option value=4>4</select><table><tr>{headers}</table><div id=scrollContent><table><tr><td>1<td>Fixture 101<td>meta{slots}</table></div>"
    );
    let parsed = parse_weekly_state(&html, 3).unwrap();
    assert_eq!(parsed.dates_of_current_week.len(), 7);
    assert_eq!(parsed.classroom_states.len(), 1);
    let row = &parsed.classroom_states[0];
    assert_eq!(row.status.len(), 42);
    assert_eq!(row.name, "Fixture 101");
    assert_eq!(row.status[0], ClassroomSlotStatus::Occupied);
    assert_eq!(row.status[5], ClassroomSlotStatus::Free);
    assert!(matches!(row.status[4], ClassroomSlotStatus::Unknown { .. }));
}
#[test]
fn backend_repair_lastmile_classroom_login_duplicate_selectors_and_depth_remain_failures() {
    let login = format!(
        "<form><input name=i_user><input name=i_pass type=password></form><div class=w30><a href='{LINK}'>Fixture</a></div>"
    );
    assert!(parse_building_list(&login).is_err());
    let duplicate = LINK.replace("weeknumber=3", "weeknumber=3&amp;weeknumber=4");
    assert!(
        parse_building_list(&format!(
            "<div class=w30><a href='{duplicate}'>Fixture</a></div>"
        ))
        .is_err()
    );
    let deep = format!(
        "{}<div class=w30><a href='{LINK}'>Fixture</a></div>{}",
        "<div>".repeat(140),
        "</div>".repeat(140)
    );
    assert!(parse_building_list(&deep).is_err());
}
