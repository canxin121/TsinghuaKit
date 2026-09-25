//! Safe academic data types exposed at the Flutter boundary.
//!
//! The Registrar parser and request profile stay in Rust. This module maps
//! their normalized result into a small DTO so the bridge never exposes HTML,
//! cookies, tickets, CSRF values, or request plans.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{
    protocol::AcademicStage,
    registrar_academic::{RegistrarCourseGrade, RegistrarGradeReport, UndergraduateReportKind},
};

/// One normalized Registrar course grade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampusGradeDto {
    pub course_name: String,
    pub credit: f64,
    pub grade: String,
    pub grade_point: Option<f64>,
    pub semester: String,
}

/// A stage-specific grade report returned by the authenticated Rust runtime.
#[derive(Debug, Clone, PartialEq)]
pub struct CampusGradeReportDto {
    pub stage: String,
    pub report_kind: Option<String>,
    pub courses: Vec<CampusGradeDto>,
    /// Provenance is optional only for compatibility with older in-process
    /// test gateways.  The Rust runtime always fills all four fields before
    /// crossing the production bridge.
    pub generated_at: Option<String>,
    pub source: Option<String>,
    pub status: Option<String>,
    pub error: Option<String>,
}

impl From<RegistrarGradeReport> for CampusGradeReportDto {
    fn from(report: RegistrarGradeReport) -> Self {
        Self {
            stage: academic_stage_name(report.stage).to_owned(),
            report_kind: report.undergraduate_report.map(report_kind_name),
            courses: report
                .courses
                .into_iter()
                .map(CampusGradeDto::from)
                .collect(),
            generated_at: None,
            source: None,
            status: None,
            error: None,
        }
    }
}

impl CampusGradeReportDto {
    /// Marks a report with the runtime-owned freshness envelope.  This keeps
    /// provenance next to the normalized records so a caller cannot mistake
    /// a stale disk payload for a live Registrar response.
    pub(crate) fn with_provenance(
        mut self,
        generated_at: String,
        source: &'static str,
        status: &'static str,
        error: Option<String>,
    ) -> Self {
        self.generated_at = Some(generated_at);
        self.source = Some(source.to_owned());
        self.status = Some(status.to_owned());
        self.error = error;
        self
    }

    pub(crate) fn live(report: RegistrarGradeReport) -> Self {
        Self::from(report).with_provenance(Utc::now().to_rfc3339(), "live", "ready", None)
    }
}

impl From<RegistrarCourseGrade> for CampusGradeDto {
    fn from(grade: RegistrarCourseGrade) -> Self {
        Self {
            course_name: grade.course_name,
            credit: grade.credit,
            grade: grade.grade,
            grade_point: grade.grade_point,
            semester: grade.semester,
        }
    }
}

fn academic_stage_name(stage: AcademicStage) -> &'static str {
    match stage {
        AcademicStage::Undergraduate => "undergraduate",
        AcademicStage::Graduate => "graduate",
    }
}

fn report_kind_name(kind: UndergraduateReportKind) -> String {
    match kind {
        UndergraduateReportKind::FirstDegree => "first_degree",
        UndergraduateReportKind::SecondDegree => "second_degree",
        UndergraduateReportKind::Minor => "minor",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_only_safe_grade_fields_to_the_bridge_dto() {
        let report = RegistrarGradeReport {
            stage: AcademicStage::Undergraduate,
            undergraduate_report: Some(UndergraduateReportKind::FirstDegree),
            courses: vec![RegistrarCourseGrade {
                course_name: "数据结构".to_owned(),
                credit: 3.0,
                grade: "A".to_owned(),
                grade_point: Some(4.0),
                semester: "2025-2026-1".to_owned(),
            }],
        };

        let mapped = CampusGradeReportDto::from(report);

        assert_eq!(mapped.stage, "undergraduate");
        assert_eq!(mapped.report_kind.as_deref(), Some("first_degree"));
        assert_eq!(mapped.courses.len(), 1);
        assert_eq!(mapped.courses[0].course_name, "数据结构");
        assert_eq!(mapped.courses[0].credit, 3.0);
        assert_eq!(mapped.courses[0].grade_point, Some(4.0));
    }
}
