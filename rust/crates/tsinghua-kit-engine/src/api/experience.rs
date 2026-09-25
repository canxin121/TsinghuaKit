//! Account-scoped presentation choices. No credentials or business records.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperiencePreferencesDto {
    pub compact: bool,
    pub auto_refresh: bool,
    pub preload: bool,
    pub section_order: Vec<String>,
    pub hidden_sections: Vec<String>,
    pub pinned_services: Vec<String>,
}

impl Default for ExperiencePreferencesDto {
    fn default() -> Self {
        Self {
            compact: false,
            auto_refresh: true,
            preload: true,
            section_order: ["schedule", "todos", "services"]
                .map(str::to_owned)
                .to_vec(),
            hidden_sections: Vec::new(),
            pinned_services: ["learn", "library", "campus_card", "electricity"]
                .map(str::to_owned)
                .to_vec(),
        }
    }
}

impl ExperiencePreferencesDto {
    pub(crate) fn validate(&self) -> Result<(), String> {
        const SECTIONS: &[&str] = &["schedule", "todos", "services"];
        const SERVICES: &[&str] = &[
            "learn",
            "registrar",
            "info",
            "library",
            "campus_card",
            "classroom",
            "electricity",
            "usereg",
            "tunet",
        ];
        let valid_list = |items: &[String], allowed: &[&str], maximum: usize| {
            items.len() <= maximum
                && items.iter().all(|item| allowed.contains(&item.as_str()))
                && items.iter().collect::<HashSet<_>>().len() == items.len()
        };
        if self.section_order.len() != SECTIONS.len()
            || !valid_list(&self.section_order, SECTIONS, 3)
            || !valid_list(&self.hidden_sections, SECTIONS, 2)
            || !valid_list(&self.pinned_services, SERVICES, 6)
            || self.pinned_services.is_empty()
        {
            return Err("首页设置无效，请重新选择".to_owned());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExperiencePreferencesPayload {
    pub account_scope: String,
    pub preferences: ExperiencePreferencesDto,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_repair_experience_preferences_reject_unknown_duplicate_and_empty_choices() {
        let defaults = ExperiencePreferencesDto::default();
        assert!(defaults.validate().is_ok());
        let mut invalid = defaults.clone();
        invalid.section_order[1] = "schedule".into();
        assert!(invalid.validate().is_err());
        invalid = defaults.clone();
        invalid.pinned_services.push("disconnect_network".into());
        assert!(invalid.validate().is_err());
        invalid = defaults.clone();
        invalid.pinned_services.clear();
        assert!(invalid.validate().is_err());
        invalid = defaults.clone();
        invalid.hidden_sections = defaults.section_order;
        assert!(invalid.validate().is_err());
    }
}
