//! Local validation scope and request-owned course evidence. Neither a cache
//! projection nor a previous report is proof of a new live business read.
use super::*;

pub(super) struct ValidationWorkspace {
    root: PathBuf,
}
impl ValidationWorkspace {
    pub(super) fn create(device_path: &Path) -> Result<Self, String> {
        let parent = device_path.parent().ok_or("validation_workspace_invalid")?;
        let root = parent.join(format!(".check-runtime-{}", Uuid::new_v4().simple()));
        // create_dir (not create_dir_all) ensures this is a newly owned scope.
        std::fs::create_dir(&root).map_err(|_| "validation_workspace_unavailable")?;
        let workspace = Self { root };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&workspace.root, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "validation_workspace_unavailable")?;
        }
        Ok(workspace)
    }
    pub(super) fn root(&self) -> &Path {
        &self.root
    }
}
impl Drop for ValidationWorkspace {
    fn drop(&mut self) {
        // Never follow a substituted directory symlink or delete any shared
        // device/app root. Only our create-new UUID directory belongs to us.
        if std::fs::symlink_metadata(&self.root)
            .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
        {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

pub(super) struct LearnValidationEvidence {
    owner: UserIdentity,
    stage: AcademicStage,
    semester: String,
    transport: crate::transport::CampusHttpTransport,
    courses: Vec<LearnCourseRecord>,
}
impl LearnValidationEvidence {
    /// Revalidate captured input provenance without issuing another request.
    pub(super) fn require_current(&self, runtime: &CampusRuntime) -> Result<(), String> {
        runtime.allow_live_operation()?;
        if !runtime.service_session_is_proven(ServiceId::Identity)
            || !runtime.service_session_is_proven(ServiceId::Learn)
            || runtime
                .coordinator
                .registry()
                .snapshot_for(ServiceId::Identity)
                .user
                .as_ref()
                != Some(&self.owner)
            || runtime
                .coordinator
                .registry()
                .snapshot_for(ServiceId::Learn)
                .user
                .as_ref()
                != Some(&self.owner)
            || runtime.semester != self.semester
            || runtime.stage != self.stage
            || !Arc::ptr_eq(
                runtime.identity.transport().cookie_jar(),
                self.transport.cookie_jar(),
            )
        {
            return Err("course_evidence_context_mismatch".into());
        }
        Ok(())
    }

    pub(super) fn ids(&self) -> Vec<String> {
        self.courses
            .iter()
            .filter_map(|course| course.course_id.clone())
            .collect()
    }
}

pub(super) fn require_live_validation_result(source: &str, status: &str) -> Result<(), String> {
    if source == "live" && status == "ready" {
        Ok(())
    } else {
        Err("validation_live_result_required".to_owned())
    }
}

impl CampusRuntime {
    /// Explicit validation reads bypass the display cache, without deleting
    /// it or mutating a global flag that can leak after cancellation.
    pub(super) async fn capture_validation_learn_courses(
        &mut self,
    ) -> Result<LearnValidationEvidence, String> {
        let result = self.read_learn_courses(false).await?;
        require_live_validation_result(&result.source, &result.status)?;
        if !self.service_session_is_proven(ServiceId::Identity)
            || !self.service_session_is_proven(ServiceId::Learn)
            || result.semester != self.semester
            || result.stage != academic_stage_name(self.stage)
        {
            return Err("course_evidence_context_mismatch".into());
        }
        let owner = self
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Identity)
            .user
            .ok_or("course_evidence_context_mismatch")?;
        let mut ids = HashSet::new();
        if result.courses.iter().any(|course| {
            course.course_id.trim().is_empty()
                || !ids.insert(course.course_id.as_str())
                || course
                    .semester
                    .as_deref()
                    .is_some_and(|semester| semester != result.semester)
        }) {
            return Err("course_evidence_context_mismatch".into());
        }
        let courses = result
            .courses
            .into_iter()
            .map(|course| LearnCourseRecord {
                course_id: Some(course.course_id),
                course_code: course.course_code,
                name: Some(course.title),
                instructor: course.instructor,
                class_name: None,
                semester: Some(result.semester.clone()),
                unknown_fields: Default::default(),
            })
            .collect();
        Ok(LearnValidationEvidence {
            owner,
            stage: self.stage,
            semester: result.semester,
            transport: self.identity.transport().clone(),
            courses,
        })
    }

    pub(super) async fn read_validation_learn_todos(
        &mut self,
        evidence: &LearnValidationEvidence,
        first_only: bool,
    ) -> Result<usize, String> {
        self.allow_live_operation()?;
        if !self.service_session_is_proven(ServiceId::Identity)
            || !self.service_session_is_proven(ServiceId::Learn)
            || self
                .coordinator
                .registry()
                .snapshot_for(ServiceId::Identity)
                .user
                .as_ref()
                != Some(&evidence.owner)
            || self
                .coordinator
                .registry()
                .snapshot_for(ServiceId::Learn)
                .user
                .as_ref()
                != Some(&evidence.owner)
            || self.semester != evidence.semester
            || self.stage != evidence.stage
            || !Arc::ptr_eq(
                self.identity.transport().cookie_jar(),
                evidence.transport.cookie_jar(),
            )
        {
            return Err("course_evidence_context_mismatch".into());
        }
        let source = self
            .learn_source
            .clone()
            .ok_or("learn_source_unavailable")?;
        if source.config().semester != evidence.semester
            || source.config().academic_stage != evidence.stage
            || !Arc::ptr_eq(
                source.registrar().transport().cookie_jar(),
                evidence.transport.cookie_jar(),
            )
        {
            return Err("course_evidence_context_mismatch".into());
        }
        let csrf = self
            .coordinator
            .bound_csrf(ServiceId::Learn)
            .ok_or("csrf_proof_missing")?;
        let courses = if first_only {
            evidence.courses.iter().take(1).cloned().collect()
        } else {
            evidence.courses.clone()
        };
        let mut todos = LearnTodoSource::new(
            source.learn().clone(),
            source.registrar().transport().clone(),
            LearnTodoConfig::new(evidence.semester.clone()).with_language("zh"),
        )
        .map_err(|_| "todo_config_failed")?
        .with_proven_courses(courses);
        todos
            .with_csrf(csrf, "_csrf")
            .map_err(|_| "csrf_proof_missing")?;
        let result = todos
            .list_todos(TodoFilter {
                include_completed: true,
                ..TodoFilter::default()
            })
            .await;
        match result {
            Ok(rows) => Ok(rows.len()),
            Err(error) => {
                if error.is_session_expired(ServiceId::Learn) {
                    self.invalidate_learn_session();
                }
                Err(error.to_string())
            }
        }
    }
}
