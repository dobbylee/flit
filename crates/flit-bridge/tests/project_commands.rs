use std::{fs, path::PathBuf, process};

use flit_bridge::{
    BridgeError, initialize_core, project_inspect_json, project_register_json, project_trust_json,
    projects_list_page_json,
};
use flit_protocol::{
    CommandError, CommandErrorCode, PROTOCOL_VERSION, ProjectInspectionResponse, ProjectListCursor,
    ProjectRegistrationResponse, ProjectRegistrationStatus, ProjectTrustResponse,
    ProjectTrustStatus, ProjectsListResponse,
};
use flit_store::MAX_PROJECT_PAGE_SIZE;

const NOW: &str = "2026-07-27T00:00:00.000Z";

fn decode<T: serde::de::DeserializeOwned>(json: String) -> T {
    serde_json::from_str(&json).expect("typed Project response")
}

fn require_command_error(
    result: Result<String, BridgeError>,
    expected_code: CommandErrorCode,
) -> CommandError {
    let error: CommandError = decode(result.expect("expected Project failure should be JSON"));
    assert_eq!(error, CommandError::for_code(expected_code));
    error
}

#[test]
fn generated_project_commands_preserve_identity_trust_duplicates_and_bounds() {
    require_command_error(
        projects_list_page_json(None, None, 1, PROTOCOL_VERSION.to_owned()),
        CommandErrorCode::StorageUnavailable,
    );

    let root = fs::canonicalize(std::env::temp_dir())
        .expect("canonical temporary root")
        .join(format!("flit-project-bridge-{}", process::id()));
    let data = root.join("data");
    let project = root.join("project");
    fs::create_dir_all(&project).expect("Project directory");
    let missing = root.join("mismatch-must-not-be-read");
    require_command_error(
        project_inspect_json(missing.to_string_lossy().into_owned(), "2.0".to_owned()),
        CommandErrorCode::ProtocolMismatch,
    );

    initialize_core(
        data.to_string_lossy().into_owned(),
        PROTOCOL_VERSION.to_owned(),
    )
    .expect("Core initializes");

    require_command_error(
        project_inspect_json(
            missing.to_string_lossy().into_owned(),
            PROTOCOL_VERSION.to_owned(),
        ),
        CommandErrorCode::ProjectInspectionFailure,
    );

    let direct: ProjectInspectionResponse = decode(
        project_inspect_json(
            project.to_string_lossy().into_owned(),
            PROTOCOL_VERSION.to_owned(),
        )
        .expect("inspect Project"),
    );
    assert_eq!(direct.protocol_version, PROTOCOL_VERSION);
    assert!(!direct.selected_via_symlink);
    assert!(PathBuf::from(&direct.canonical_path).is_absolute());

    let selected_link = root.join("selected-link");
    std::os::unix::fs::symlink(&project, &selected_link).expect("Project symlink");
    let linked: ProjectInspectionResponse = decode(
        project_inspect_json(
            selected_link.to_string_lossy().into_owned(),
            PROTOCOL_VERSION.to_owned(),
        )
        .expect("inspect Project symlink"),
    );
    assert!(linked.selected_via_symlink);
    assert_eq!(linked.canonical_path, direct.canonical_path);
    assert_eq!(linked.filesystem_id, direct.filesystem_id);

    let registered: ProjectRegistrationResponse = decode(
        project_register_json(
            "project-one".to_owned(),
            "Project One".to_owned(),
            project.to_string_lossy().into_owned(),
            NOW.to_owned(),
            PROTOCOL_VERSION.to_owned(),
        )
        .expect("register Project"),
    );
    assert_eq!(registered.status, ProjectRegistrationStatus::Registered);
    assert_eq!(
        registered.project.as_ref().map(|value| value.trusted),
        Some(false)
    );

    let duplicate: ProjectRegistrationResponse = decode(
        project_register_json(
            "project-two".to_owned(),
            "Project Two".to_owned(),
            selected_link.to_string_lossy().into_owned(),
            NOW.to_owned(),
            PROTOCOL_VERSION.to_owned(),
        )
        .expect("duplicate Project"),
    );
    assert_eq!(
        duplicate.status,
        ProjectRegistrationStatus::DuplicateCanonicalPath
    );
    assert_eq!(
        duplicate.existing_project_id.as_deref(),
        Some("project-one")
    );

    let conflict_path = root.join("conflict");
    fs::create_dir(&conflict_path).expect("conflicting Project directory");
    require_command_error(
        project_register_json(
            "project-one".to_owned(),
            "Conflicting Project".to_owned(),
            conflict_path.to_string_lossy().into_owned(),
            NOW.to_owned(),
            PROTOCOL_VERSION.to_owned(),
        ),
        CommandErrorCode::ProjectConflict,
    );
    require_command_error(
        project_trust_json(
            "missing-project".to_owned(),
            project.to_string_lossy().into_owned(),
            NOW.to_owned(),
            PROTOCOL_VERSION.to_owned(),
        ),
        CommandErrorCode::ProjectNotFound,
    );

    let trusted: ProjectTrustResponse = decode(
        project_trust_json(
            "project-one".to_owned(),
            project.to_string_lossy().into_owned(),
            NOW.to_owned(),
            PROTOCOL_VERSION.to_owned(),
        )
        .expect("trust Project"),
    );
    assert_eq!(trusted.status, ProjectTrustStatus::Trusted);
    assert!(trusted.project.trusted);
    let repeated: ProjectTrustResponse = decode(
        project_trust_json(
            "project-one".to_owned(),
            project.to_string_lossy().into_owned(),
            NOW.to_owned(),
            PROTOCOL_VERSION.to_owned(),
        )
        .expect("repeat trust"),
    );
    assert_eq!(repeated.status, ProjectTrustStatus::AlreadyTrusted);

    let listed: ProjectsListResponse = decode(
        projects_list_page_json(None, None, 50, PROTOCOL_VERSION.to_owned())
            .expect("list Projects"),
    );
    assert_eq!(listed.projects.len(), 1);
    assert_eq!(listed.projects[0].id, "project-one");
    assert!(listed.projects[0].trusted);
    assert_eq!(listed.next_cursor, None);

    require_command_error(
        projects_list_page_json(
            Some("Project One".to_owned()),
            None,
            50,
            PROTOCOL_VERSION.to_owned(),
        ),
        CommandErrorCode::InvalidProjectRequest,
    );
    require_command_error(
        projects_list_page_json(
            None,
            None,
            u32::try_from(MAX_PROJECT_PAGE_SIZE + 1).unwrap(),
            PROTOCOL_VERSION.to_owned(),
        ),
        CommandErrorCode::InvalidProjectRequest,
    );

    for index in 0..MAX_PROJECT_PAGE_SIZE + 3 {
        let paged_path = root.join(format!("paged-{index:03}"));
        fs::create_dir(&paged_path).expect("paged Project directory");
        let response: ProjectRegistrationResponse = decode(
            project_register_json(
                format!("paged-{index:03}"),
                format!("Paged {index:03}"),
                paged_path.to_string_lossy().into_owned(),
                NOW.to_owned(),
                PROTOCOL_VERSION.to_owned(),
            )
            .expect("register paged Project"),
        );
        assert_eq!(response.status, ProjectRegistrationStatus::Registered);
    }

    let mut cursor: Option<ProjectListCursor> = None;
    let mut listed_ids = Vec::new();
    let mut page_sizes = Vec::new();
    loop {
        let page: ProjectsListResponse = decode(
            projects_list_page_json(
                cursor.as_ref().map(|value| value.display_name.clone()),
                cursor.as_ref().map(|value| value.project_id.clone()),
                u32::try_from(MAX_PROJECT_PAGE_SIZE).unwrap(),
                PROTOCOL_VERSION.to_owned(),
            )
            .expect("list bounded Project page"),
        );
        page_sizes.push(page.projects.len());
        assert!(page.projects.len() <= MAX_PROJECT_PAGE_SIZE);
        listed_ids.extend(page.projects.into_iter().map(|project| project.id));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(page_sizes, [MAX_PROJECT_PAGE_SIZE, 4]);
    assert_eq!(listed_ids.len(), MAX_PROJECT_PAGE_SIZE + 4);
    listed_ids.sort();
    listed_ids.dedup();
    assert_eq!(listed_ids.len(), MAX_PROJECT_PAGE_SIZE + 4);

    require_command_error(
        project_register_json(
            "x".repeat(129),
            "Oversized".to_owned(),
            missing.to_string_lossy().into_owned(),
            NOW.to_owned(),
            PROTOCOL_VERSION.to_owned(),
        ),
        CommandErrorCode::InvalidProjectRequest,
    );
    assert!(!missing.exists());

    fs::rename(&project, root.join("moved-project")).expect("move Project");
    fs::create_dir(&project).expect("replacement Project");
    require_command_error(
        project_trust_json(
            "project-one".to_owned(),
            project.to_string_lossy().into_owned(),
            NOW.to_owned(),
            PROTOCOL_VERSION.to_owned(),
        ),
        CommandErrorCode::ProjectIdentityMismatch,
    );

    fs::remove_dir_all(root).expect("remove exact test root");
}
