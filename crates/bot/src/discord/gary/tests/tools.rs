use crate::agones::{DestroyOutcome, EditOutcome, FsOutcome, ReadyWait, SupervisorOutcome};

use super::*;

fn tool_names(access: AccessLevel) -> Vec<String> {
    available_tools(access)
        .into_iter()
        .map(|tool| tool.function.name)
        .collect()
}

/// The tools reserved for admins — never offered to a manager or read-only
/// caller. Kept next to the tier tests so a newly added admin tool is checked.
const ADMIN_ONLY: [&str; 5] = [
    DestroyServer::NAME,
    SendCommand::NAME,
    ArchiveServer::NAME,
    RestoreServer::NAME,
    RecoverServer::NAME,
];

/// The lifecycle, file, discovery, and memory tools a manager gets on top of the
/// read-only set.
const MANAGER_ADDED: [&str; 17] = [
    CreateServer::NAME,
    StopServer::NAME,
    StartServer::NAME,
    RestartServer::NAME,
    ShutdownServer::NAME,
    BrowseFiles::NAME,
    ReadFile::NAME,
    ReadLogs::NAME,
    WriteFile::NAME,
    EditFile::NAME,
    RestoreFile::NAME,
    RunWhen::NAME,
    BackupServer::NAME,
    SearchMods::NAME,
    ModInfo::NAME,
    Remember::NAME,
    Forget::NAME,
];

#[test]
fn read_only_callers_get_only_the_read_only_tools() {
    let names = tool_names(AccessLevel::ReadOnly);
    assert_eq!(
        names,
        vec![
            ListServers::NAME,
            ServerStatus::NAME,
            ListBackups::NAME,
            ListArchives::NAME
        ]
    );
    for reserved in MANAGER_ADDED.iter().chain(ADMIN_ONLY.iter()) {
        assert!(
            !names.iter().any(|name| name == reserved),
            "read-only tier must not expose {reserved}"
        );
    }
}

#[test]
fn managers_get_lifecycle_and_files_but_not_the_destructive_tools() {
    let names = tool_names(AccessLevel::Manager);
    for expected in MANAGER_ADDED {
        assert!(
            names.iter().any(|name| name == expected),
            "managers should get {expected}"
        );
    }
    for reserved in ADMIN_ONLY {
        assert!(
            !names.iter().any(|name| name == reserved),
            "{reserved} must not be offered to managers"
        );
    }
    assert_eq!(
        names.len(),
        21,
        "four read tools plus the seventeen manager lifecycle/file/discovery/memory tools"
    );
}

#[test]
fn admins_get_the_full_lifecycle_and_filesystem_set() {
    let names = tool_names(AccessLevel::Admin);
    for expected in MANAGER_ADDED.iter().chain(ADMIN_ONLY.iter()) {
        assert!(
            names.iter().any(|name| name == expected),
            "missing {expected}"
        );
    }
    assert_eq!(
        names.len(),
        26,
        "the four read tools, seventeen manager tools, and five admin-only tools"
    );
}

#[test]
fn tier_refusal_names_the_tier_each_tool_actually_needs() {
    // Walks the live table so the refusal prose can't be wired to the wrong tier:
    // telling a read-only caller "no such tool" when the real answer is "an admin
    // has to do that" hides the actual reason and sends Gary down a dead end.
    for (tier, tool) in tool_table() {
        let name = tool.function.name;
        let expected = match tier {
            AccessLevel::Admin => {
                "that action needs an admin — I can only look things up or run day-to-day changes for you here.".to_owned()
            }
            AccessLevel::Manager => {
                "that action needs a manager or an admin — I can only look things up for you here."
                    .to_owned()
            }
            // Read-only tools sit below every caller's tier, so they can never be
            // refused for tier; asking anyway is asking about a name, not a tier.
            AccessLevel::ReadOnly => format!("'{name}' isn't a tool I have."),
        };
        assert_eq!(tier_refusal(&name), expected, "{name}: wrong tier refusal");
    }
}

#[test]
fn an_unknown_name_has_no_tier_and_reads_as_unknown() {
    assert_eq!(tier_of("frobnicate"), None);
    assert!(!known_tool("frobnicate"));
    assert_eq!(
        tier_refusal("frobnicate"),
        "'frobnicate' isn't a tool I have."
    );
}

#[test]
fn every_offered_tool_is_known_and_within_the_callers_tier() {
    // The offer list and the dispatch gate read the same table; this pins that
    // they agree, so a caller is never offered a tool the gate would refuse.
    for access in [
        AccessLevel::ReadOnly,
        AccessLevel::Manager,
        AccessLevel::Admin,
    ] {
        for name in tool_names(access) {
            assert!(known_tool(&name), "{name} is offered but not a known tool");
            let required = tier_of(&name).expect("offered tool has a tier");
            assert!(
                access >= required,
                "{name} was offered to {access:?} but needs {required:?}"
            );
        }
    }
}

#[test]
fn scope_gate_covers_every_targeted_tool_and_spares_list_and_create() {
    // The gate must apply to every tool that names an existing server, and only
    // to those — list_servers/list_archives scope their own query, and
    // create_server/recover_server make a new server stamped with the channel.
    // Derived from the live tool set so a newly added tool can't slip past the
    // gate without this failing.
    for name in tool_names(AccessLevel::Admin) {
        // remember/forget carry no server name (memory is cross-guild), and the
        // Modrinth lookups read a public registry rather than the cluster, so
        // they're spared the gate alongside the listing/create tools.
        let should_gate = name != ListServers::NAME
            && name != CreateServer::NAME
            && name != ListArchives::NAME
            && name != RecoverServer::NAME
            && name != SearchMods::NAME
            && name != ModInfo::NAME
            && name != Remember::NAME
            && name != Forget::NAME;
        assert_eq!(
            targets_existing_server(&name),
            should_gate,
            "{name}: scope-gating classification is wrong"
        );
    }
}

#[test]
fn unknown_tool_is_not_scope_gated() {
    assert!(!targets_existing_server("frobnicate"));
}

#[test]
fn filesystem_tools_are_manager_and_up() {
    // File tools power day-to-day troubleshooting, so managers get them — but
    // read-only callers never do.
    let read_only = tool_names(AccessLevel::ReadOnly);
    let managers = tool_names(AccessLevel::Manager);
    for tool in [
        BrowseFiles::NAME,
        ReadFile::NAME,
        ReadLogs::NAME,
        WriteFile::NAME,
        EditFile::NAME,
        RestoreFile::NAME,
        RunWhen::NAME,
    ] {
        assert!(
            !read_only.iter().any(|name| name == tool),
            "{tool} must not be offered to read-only callers"
        );
        assert!(
            managers.iter().any(|name| name == tool),
            "{tool} should be offered to managers"
        );
    }
}

#[test]
fn send_command_is_admin_only() {
    for tier in [AccessLevel::ReadOnly, AccessLevel::Manager] {
        assert!(
            !tool_names(tier)
                .iter()
                .any(|name| name == SendCommand::NAME),
            "send_command must not be offered below the admin tier"
        );
    }
    assert!(
        tool_names(AccessLevel::Admin)
            .iter()
            .any(|name| name == SendCommand::NAME),
        "send_command must be offered to admins"
    );
}

#[test]
fn command_param_schema_exposes_name_and_command() {
    let schema = SendCommand::spec().parameters;
    let properties = schema
        .as_object()
        .and_then(|object| object.get("properties"))
        .and_then(serde_json::Value::as_object)
        .unwrap();
    assert!(properties.contains_key("name"), "schema needs a name field");
    assert!(
        properties.contains_key("command"),
        "schema needs a command field"
    );
}

#[test]
fn run_when_condition_schema_is_a_closed_enum() {
    let schema = RunWhen::spec().parameters;
    assert_eq!(
        schema.pointer("/properties/condition/type"),
        Some(&serde_json::Value::from("string")),
        "an enum parameter is a string on the wire"
    );
    assert_eq!(
        schema.pointer("/properties/condition/enum"),
        Some(&serde_json::json!(["startup", "empty", "idle"])),
        "the closed condition set must reach the model as an enum array"
    );
}

#[test]
fn run_when_rejects_an_unknown_condition() {
    // A condition outside the generated enum fails deserialization, and the model
    // gets the explanatory retry message rather than the loop erroring out.
    let message = parse::<RunWhenParams>(r#"{"name":"mc","condition":"whenever","task":"x"}"#)
        .err()
        .expect("an unknown condition must not deserialize");
    assert!(
        message.contains("weren't valid JSON"),
        "the model should get the retry hint, got: {message}"
    );
}

#[test]
fn narrow_lines_passes_through_and_refuses_negatives() {
    assert_eq!(narrow_lines(None), Ok(None), "omitted lines stays absent");
    assert_eq!(
        narrow_lines(Some(50)),
        Ok(Some(50)),
        "a positive count narrows"
    );
    let refused = narrow_lines(Some(-3)).expect_err("a negative count must be refused");
    assert!(
        refused.contains("can't be negative"),
        "the model should get an actionable message, got: {refused}"
    );
}

#[test]
fn command_output_renders_reply_or_notes_silence() {
    let with_output = CommandResponse {
        output: "There are 2 of a max of 20 players online".to_owned(),
    };
    let rendered = format_command_output("mc", "list", &with_output);
    assert!(rendered.contains("list"));
    assert!(rendered.contains("There are 2 of a max of 20 players online"));

    let empty = CommandResponse {
        output: "   \n".to_owned(),
    };
    assert!(
        format_command_output("mc", "say hi", &empty).contains("no output"),
        "a blank reply should be reported as no output"
    );
}

#[test]
fn name_param_schema_is_clean_object() {
    // A NameParams-backed tool: the assembled schema is a bare object with the
    // name field, additionalProperties locked off, and none of the metadata keys
    // some providers reject (the generator never emits $schema/title).
    let schema = ServerStatus::spec().parameters;
    let object = schema.as_object().unwrap();
    assert_eq!(
        object.get("type").and_then(serde_json::Value::as_str),
        Some("object")
    );
    assert!(
        object
            .get("properties")
            .and_then(|properties| properties.get("name"))
            .is_some(),
        "the name parameter must be in the schema"
    );
    assert_eq!(
        object.get("additionalProperties"),
        Some(&serde_json::Value::Bool(false)),
        "the assembled schema locks additionalProperties off"
    );
    assert!(
        !object.contains_key("$schema") && !object.contains_key("title"),
        "provider-unfriendly metadata keys are never generated"
    );
}

#[test]
fn name_params_round_trips_from_tool_arguments() {
    let parsed: NameParams = serde_json::from_str(r#"{"name":"mc-abc123"}"#).unwrap();
    assert_eq!(parsed.name, "mc-abc123");
}

#[test]
fn supervisor_outcomes_map_to_distinct_messages() {
    let paused = format_supervisor("mc", &SupervisorOutcome::Paused);
    let running = format_supervisor("mc", &SupervisorOutcome::AlreadyRunning);
    let missing = format_supervisor("mc", &SupervisorOutcome::NotFound);
    assert!(paused.contains("paused"));
    assert!(running.contains("already running"));
    assert_eq!(
        missing,
        "there's no server named mc — check list_servers for the current names"
    );
}

#[test]
fn supervisor_failed_relays_the_supervisors_reason() {
    let message = format_supervisor(
        "mc",
        &SupervisorOutcome::Failed("rcon is not enabled for this game".to_owned()),
    );
    assert!(
        message.contains("rcon is not enabled for this game"),
        "Gary should relay the supervisor's own reason, got: {message}"
    );
}

#[test]
fn remove_outcomes_report_deletion_or_absence() {
    assert_eq!(
        format_destroy("mc", &DestroyOutcome::Destroyed),
        "deleted mc and its world"
    );
    assert!(format_destroy("mc", &DestroyOutcome::NotManaged).contains("managed by the platform"));
}

#[test]
fn fs_result_passes_payload_through_and_maps_problems() {
    assert_eq!(fs_result("mc", FsOutcome::Ok(42)), Ok(42));
    assert_eq!(
        fs_result::<()>("mc", FsOutcome::NotFound),
        Err("there's no server named mc — check list_servers for the current names".to_owned())
    );
    assert!(
        fs_result::<()>("mc", FsOutcome::NotManaged)
            .unwrap_err()
            .contains("managed by the platform")
    );
    assert!(
        fs_result::<()>("mc", FsOutcome::PodNotReady)
            .unwrap_err()
            .contains("isn't ready")
    );
    assert!(
        fs_result::<()>("mc", FsOutcome::Unreachable)
            .unwrap_err()
            .contains("couldn't reach")
    );
    assert_eq!(
        fs_result::<()>(
            "mc",
            FsOutcome::Rejected("path is outside the server's data directory".to_owned())
        ),
        Err("that didn't work: path is outside the server's data directory".to_owned()),
        "a supervisor rejection should be relayed verbatim after the lead-in"
    );
}

#[test]
fn browse_listing_describes_files_and_folders() {
    let entries = vec![
        DirEntry {
            name: "logs".to_owned(),
            kind: EntryKind::Dir,
            size_bytes: 0,
        },
        DirEntry {
            name: "server.properties".to_owned(),
            kind: EntryKind::File,
            size_bytes: 1024,
        },
    ];
    let rendered = format_entries("", &entries);
    assert!(rendered.contains("the data directory"));
    assert!(rendered.contains("logs/ (folder)"));
    assert!(rendered.contains("server.properties (1024 bytes)"));
}

#[test]
fn empty_directory_is_reported() {
    assert_eq!(format_entries("config", &[]), "config is empty");
}

fn terralith_hit() -> SearchHit {
    serde_json::from_str(
        r#"{"slug":"terralith","title":"Terralith","description":"Adds over 95 biomes.",
            "downloads":20538127,"client_side":"optional","server_side":"required",
            "categories":["datapack","fabric","forge","worldgen"],
            "versions":["1.18","26.1","26.2"],"latest_version":"EyCqftOK"}"#,
    )
    .unwrap()
}

#[test]
fn a_mod_line_leads_with_the_slug_and_names_both_install_sides() {
    // The slug leads because it's the only field copied verbatim into a server's
    // config; both side flags appear because a client-required project means
    // every player has work to do that no server change can cover.
    let line = format_mod_line(&terralith_hit(), None);
    assert!(line.starts_with("terralith — Terralith:"), "{line}");
    assert!(line.contains("needed on the server: required"), "{line}");
    assert!(
        line.contains("needed on players' machines: optional"),
        "{line}"
    );
    assert!(line.contains("downloads: 20538127"), "{line}");
}

#[test]
fn a_mod_line_lists_loaders_without_the_genuine_categories() {
    // Loaders come back mixed into a hit's categories alongside real tags, so a
    // sloppy split would advertise "worldgen" as something to set TYPE to.
    let line = format_mod_line(&terralith_hit(), None);
    assert!(line.contains("loaders: datapack, fabric, forge"), "{line}");
    assert!(!line.contains("worldgen"), "{line}");
}

#[test]
fn a_search_listing_heads_the_hits_and_says_it_is_capped() {
    let rendered = format_mod_search("world generation", None, &[terralith_hit()]);
    assert!(rendered.contains("for world generation"), "{rendered}");
    assert!(rendered.contains("best five only"), "{rendered}");
    assert!(rendered.contains("terralith — Terralith:"), "{rendered}");
}

#[test]
fn no_matches_reads_as_empty_rather_than_broken() {
    // An empty result must not look like an outage, or the model reports the
    // registry down instead of widening the search.
    let rendered = format_mod_search("nonexistent thing", None, &[]);
    assert!(
        rendered.contains("nothing on Modrinth matched"),
        "{rendered}"
    );
    assert!(rendered.contains("nonexistent thing"), "{rendered}");
    assert!(!rendered.contains("couldn't reach"), "{rendered}");
}

#[test]
fn project_detail_reports_its_own_loaders_and_states_the_version_rule() {
    let project: Project = serde_json::from_str(
        r#"{"slug":"chunky","title":"Chunky","description":"Pre-generates chunks.",
            "downloads":1000,"client_side":"unsupported","server_side":"required",
            "loaders":["fabric","neoforge"],"game_versions":["26.1","26.2"]}"#,
    )
    .unwrap();
    let rendered = format_mod_info(&project);
    assert!(rendered.contains("loaders: fabric, neoforge"), "{rendered}");
    assert!(
        rendered.contains("recent Minecraft versions: 26.1, 26.2"),
        "{rendered}"
    );
    assert!(
        rendered.contains("stops the server from starting"),
        "the version rule must be stated, not implied: {rendered}"
    );
}

#[test]
fn a_missing_slug_steers_back_to_search_instead_of_installing() {
    // This is the case mod_info exists to catch: a guessed slug written into the
    // config fails the boot, and the replacement pod re-reads it and fails too.
    let rendered = prompts::ModNotFound { slug: "terrilith" }.render();
    assert!(rendered.contains("terrilith"), "{rendered}");
    assert!(rendered.contains("search_mods"), "{rendered}");
    assert!(rendered.contains("don't install this one"), "{rendered}");
}

#[test]
fn an_unreachable_registry_does_not_read_as_a_missing_mod() {
    let rendered = modrinth_unavailable();
    assert!(rendered.contains("couldn't reach Modrinth"), "{rendered}");
    assert!(
        rendered.contains("doesn't mean the mod is missing"),
        "{rendered}"
    );
}

#[test]
fn the_search_schema_offers_closed_enums_for_kind_and_loader() {
    // Open strings would let the model invent a project type or loader the
    // registry doesn't index, which silently returns nothing.
    let schema = SearchMods::spec().parameters;
    let kinds = schema
        .pointer("/properties/kind/enum")
        .and_then(serde_json::Value::as_array)
        .expect("kind is an enum");
    assert_eq!(
        kinds,
        &[serde_json::json!("mod"), serde_json::json!("datapack")]
    );
    let loaders = schema
        .pointer("/properties/loader/enum")
        .and_then(serde_json::Value::as_array)
        .expect("loader is an enum");
    assert_eq!(
        loaders,
        &[
            serde_json::json!("fabric"),
            serde_json::json!("forge"),
            serde_json::json!("neoforge"),
            serde_json::json!("quilt")
        ]
    );
}

#[test]
fn only_the_search_query_is_required() {
    let schema = SearchMods::spec().parameters;
    let required = schema
        .pointer("/required")
        .and_then(serde_json::Value::as_array)
        .expect("the schema names its required params");
    assert_eq!(required, &[serde_json::json!("query")]);
}

#[test]
fn discovery_tools_are_manager_and_up() {
    let read_only = tool_names(AccessLevel::ReadOnly);
    let managers = tool_names(AccessLevel::Manager);
    for tool in [SearchMods::NAME, ModInfo::NAME] {
        assert!(
            !read_only.iter().any(|name| name == tool),
            "{tool} must not be offered to read-only callers"
        );
        assert!(
            managers.iter().any(|name| name == tool),
            "{tool} should be offered to managers"
        );
    }
}

#[test]
fn read_file_notes_truncation() {
    let whole = ReadResponse {
        path: "server.properties".to_owned(),
        content: "difficulty=hard".to_owned(),
        truncated: false,
    };
    assert!(!format_file(&whole).contains("truncated"));
    let cut = ReadResponse {
        path: "logs/latest.log".to_owned(),
        content: "...".to_owned(),
        truncated: true,
    };
    assert!(format_file(&cut).contains("truncated"));
}

#[test]
fn write_result_flags_whether_a_revert_is_possible() {
    let overwrite = WriteResponse {
        path: "server.properties".to_owned(),
        backed_up: true,
    };
    let rendered = format_write(&overwrite);
    assert!(rendered.contains("restore_file"));
    assert!(rendered.contains("restart"));
    let fresh = WriteResponse {
        path: "ops.json".to_owned(),
        backed_up: false,
    };
    assert!(format_write(&fresh).contains("nothing to restore"));
}

#[test]
fn edit_success_points_at_restart_and_undo() {
    let rendered = format_edit(
        "mc",
        "server.properties",
        EditOutcome::Edited(WriteResponse {
            path: "server.properties".to_owned(),
            backed_up: true,
        }),
    );
    assert!(rendered.contains("edited server.properties"));
    assert!(
        rendered.contains("restore_file"),
        "should mention how to undo"
    );
    assert!(rendered.contains("restart"), "should prompt a restart");
}

#[test]
fn edit_soft_failures_explain_the_recovery() {
    assert!(
        format_edit("mc", "server.properties", EditOutcome::NoMatch).contains("couldn't find"),
        "a missing anchor should tell Gary to re-read and match exactly"
    );
    let ambiguous = format_edit("mc", "server.properties", EditOutcome::Ambiguous(3));
    assert!(ambiguous.contains('3'), "ambiguity should report the count");
    assert!(
        format_edit("mc", "server.properties", EditOutcome::TooLargeToEdit).contains("write_file"),
        "an un-editable large file should point at the write_file fallback"
    );
    // A shared FS failure carried through Unserved renders like any other.
    assert_eq!(
        format_edit(
            "mc",
            "server.properties",
            EditOutcome::Unserved(FsOutcome::NotFound)
        ),
        "there's no server named mc — check list_servers for the current names"
    );
}

#[test]
fn ready_wait_outcomes_map_to_distinct_messages() {
    assert!(format_ready_wait("mc", &ReadyWait::Ready).contains("back up"));
    assert!(format_ready_wait("mc", &ReadyWait::Crashed).contains("crashed"));
    assert!(
        format_ready_wait("mc", &ReadyWait::Stopped).contains("stopped"),
        "a paused server won't come up on its own and should say so"
    );
    assert!(format_ready_wait("mc", &ReadyWait::TimedOut).contains("still isn't accepting"));
    assert_eq!(
        format_ready_wait("mc", &ReadyWait::NotFound),
        "there's no server named mc — check list_servers for the current names"
    );
}

#[test]
fn logs_render_or_report_absence() {
    assert!(format_logs("mc", &[]).contains("hasn't produced any output"));
    let rendered = format_logs(
        "mc",
        &["[12:00] starting".to_owned(), "[12:01] ready".to_owned()],
    );
    assert!(rendered.contains("[12:00] starting"));
    assert!(rendered.contains("[12:01] ready"));
}

#[test]
fn confirmations_default_to_a_human_click() {
    // The whole safety story rests on a new surface having to *ask* for
    // pre-approval. If the default ever flips, every caller that doesn't spell
    // its posture out inherits the ability to destroy a world unprompted.
    assert_eq!(ConfirmMode::default(), ConfirmMode::Interactive);
}
