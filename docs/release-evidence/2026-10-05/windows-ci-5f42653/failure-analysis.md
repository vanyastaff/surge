# Windows 5f42653 failure inventory against dd8d2b0

Read-only extraction of actual completed job logs; no builds or code edits. Each failed test is counted once, excluding nextest summary repetitions.

5f42653: **3705 run, 3516 passed, 189 failed, 48 skipped**. dd8d2b0: **3703 run, 3505 passed, 198 failed, 47 skipped**. The separate standard-user probe step improved from 13/13 to 14/14 PASS; this does not validate privileged fixtures or the whole Windows suite.

| Failure family (observed) | dd8 | 5f |
|---|---:|---:|
| acp-drop-pid-cleanup | 1 | 0 |
| lock-os33 | 10 | 0 |
| mcp-cold-readiness-elapsed | 5 | 5 |
| missing-provider-container-identity | 4 | 4 |
| owner-security-refusal | 17 | 17 |
| preparation-child-before-lock | 1 | 1 |
| private-preparation-unsupported | 22 | 22 |
| registry-contention-deadline | 1 | 0 |
| sharing-os32 | 137 | 138 |
| suspension-attention-mismatch | 0 | 2 |

OS32 is an observed sharing/cleanup symptom. The log alone does not prove every one of these 138 tests has the same retained owner. The local engine-forwarder investigation is separate causal evidence; its unpushed repairs are absent from 5f42653.

## Exact changed failures

**187 names remain failed; 11 previously failing tests now pass; 2 previously passing tests fail with OS32.**

Newly failing:
- `surge-daemon::work_item_route_test killed_continue_owner_after_journal_ack_before_registry_ack_routes_once`
- `surge-orchestrator::elevation_blocked resolve_elevation_returns_run_not_found_for_unknown_run`

Now PASS (actual PASS rows verified):
- `surge-acp pool::tests::shutdown::test_drop_cleanup`
- `surge-cli::cli_run_lifecycle daemon_gate_remains_pending_until_explicit_suspension`
- `surge-cli::cli_run_lifecycle foreground_failure_is_nonzero_and_durable`
- `surge-cli::cli_run_lifecycle foreground_success_is_durable`
- `surge-cli::cli_run_lifecycle user_template_executes_to_completion`
- `surge-cli::cli_run_lifecycle watched_failure_is_nonzero_and_durable`
- `surge-cli::cli_run_lifecycle watched_success_is_durable`
- `surge-cli::examples_smoke onboarding_smoke_can_init_describe_and_start_example_run`
- `surge-cli::fault_injection unclean_exit_mid_run_preserves_and_folds_event_log`
- `surge-daemon pidfile::windows::tests::guard_blocks_second_owner_then_releases_original_file`
- `surge-daemon pidfile::windows::tests::terminated_process_with_exit_259_is_stale_even_with_retained_handle`

Changed symptom within shared failures:

- `work_item_route_test::task_continue_without_provider_restore_capability_preserves_attention` and `task_suspend_continue_uses_saved_provider_on_real_acp_wire`: previous OS32 cleanup panic, now `Attention != Suspended` at source line 3240. Both actual diagnostics say `task conflict: storage error: suspended task requires its durable Continue reservation`, with journal `cleanup_confirmed: true`. Do not count as new test failures; the log establishes this diagnostic, not a completed causal investigation.
- `engine_capacity_park_test::registry_contention_does_not_delay_route_commit_behind_capacity_clear`: previous route-lock timing assertion (2.5416416s), now OS32 cleanup. Its earlier timing failure did not recur in this execution; this is not a timing-fix proof.

## Boundaries / narrower source findings

- 22 explicit unsupported-preparation failures: 15 direct quota-route assertions + one child readiness failure with the same captured preparation error, and six persistence preparation tests. Guards remain intentionally closed pending reviewed private/preparation stages.
- 17 actual owner refusals: eleven daemon fixtures hit `create_run` after prior ArtifactStore::put (work_item_route_test.rs690–701); four orchestrator integration tests use generic temp `.db` paths for Store::open; two preparation fixtures refuse at work_items.rs1318. These need bounded owning-fixture audit, not a trust-policy relaxation.
- Five owned_flow_mcp_recovery cases fail child wait_file at line400 (30-second awaited path); readiness failure is observed, underlying missing producer not established by this log.
- Four cold-route tests fail at line2779: missing SessionOpened.opened.execution_writer.container identity, not an arbitrary missing journal event.
- One preparation owner child exits before owner.ready; its stdout and stderr are explicitly null at work_items.rs1600–1601, so its root cause cannot honestly be assigned from this log.

## All 189 exact failed names

### mcp-cold-readiness-elapsed

- `surge-daemon::owned_flow_mcp_recovery explicit_empty_survives_real_cold_wake_and_changed_global_defaults` — raw log line 3804
- `surge-daemon::owned_flow_mcp_recovery host_default_empty_survives_real_cold_wake_and_changed_global_defaults` — raw log line 3733
- `surge-daemon::owned_flow_mcp_recovery legacy_engine_catalog_enforces_actual_configured_deadline` — raw log line 3875
- `surge-daemon::owned_flow_mcp_recovery legacy_global_policy_references_survive_actual_start_and_resume` — raw log line 3944
- `surge-daemon::owned_flow_mcp_recovery optional_whitelists_and_server_override_survive_actual_start_and_resume` — raw log line 6574

### missing-provider-container-identity

- `surge-daemon::work_item_route_test cold_committed_outcome_does_not_repeat_a_real_provider_turn` — raw log line 5357
- `surge-daemon::work_item_route_test cold_consumed_route_before_registry_ack_does_not_route_twice` — raw log line 5423
- `surge-daemon::work_item_route_test cold_consumed_route_corrupt_checkpoint_keeps_attention_without_provider_dispatch` — raw log line 5489
- `surge-daemon::work_item_route_test cold_consumed_route_missing_checkpoint_keeps_attention_without_provider_dispatch` — raw log line 5683

### owner-security-refusal

- `surge-daemon::work_item_route_test ambiguous_owned_terminal_history_never_releases_task_ownership` — raw log line 5291
- `surge-daemon::work_item_route_test cold_daemon_keeps_missing_binding_in_attention_and_never_dispatches` — raw log line 5555
- `surge-daemon::work_item_route_test cold_daemon_resumes_committed_startup_with_same_run_and_accepted_acp_context` — raw log line 5619
- `surge-daemon::work_item_route_test corrupt_owned_startup_graph_payload_cannot_reuse_declared_hash` — raw log line 5749
- `surge-daemon::work_item_route_test corrupt_owned_startup_late_binding_after_terminal_keeps_ownership` — raw log line 5939
- `surge-daemon::work_item_route_test corrupt_owned_startup_late_binding_before_terminal_is_attention` — raw log line 6005
- `surge-daemon::work_item_route_test corrupt_owned_startup_late_prompt_artifact_is_attention` — raw log line 6071
- `surge-daemon::work_item_route_test corrupt_owned_startup_late_requirements_artifact_is_attention` — raw log line 6137
- `surge-daemon::work_item_route_test durable_terminal_settlement_validates_frozen_requirements_before_releasing_ownership` — raw log line 6203
- `surge-daemon::work_item_route_test legacy_unbound_cap_resumes_generic_task_but_current_missing_cap_retains_attention` — raw log line 6705
- `surge-daemon::work_item_route_test partial_task_startup_missing_initial_prompt_is_attention_without_dispatch` — raw log line 7079
- `surge-orchestrator::integration test_cost_accumulation` — raw log line 13188
- `surge-orchestrator::integration test_cost_calculation_preservation` — raw log line 13271
- `surge-orchestrator::integration test_session_without_subtask` — raw log line 13440
- `surge-orchestrator::integration test_token_tracking_end_to_end` — raw log line 13360
- `surge-persistence work_items::tests::start_preparation_consumes_once_and_replay_does_no_filesystem_work` — raw log line 14809
- `surge-persistence work_items::tests::start_preparation_relocated_live_lock_never_proves_dead_ownership` — raw log line 15116

### preparation-child-before-lock

- `surge-persistence work_items::tests::start_preparation_killed_owner_can_be_reclaimed_without_provider_authority` — raw log line 15180

### private-preparation-unsupported

- `surge-daemon::quota_recovery_route_test actual_429_after_declared_source_change_is_audit_only_and_primary_is_attempted_again` — raw log line 4020
- `surge-daemon::quota_recovery_route_test normal_task_all_fresh_exhausted_parks_without_opening_any_provider` — raw log line 4089
- `surge-daemon::quota_recovery_route_test normal_task_mixed_skip_and_actual_429_never_reopens_primary` — raw log line 4161
- `surge-daemon::quota_recovery_route_test normal_task_start_skips_fresh_exhausted_primary_before_any_provider_effect` — raw log line 4291
- `surge-daemon::quota_recovery_route_test pinned_exhaustion_from_other_project_cannot_skip_primary` — raw log line 4431
- `surge-daemon::quota_recovery_route_test planned_host_budget_positive_cap_completes_without_repeating_warmup` — raw log line 4363
- `surge-daemon::quota_recovery_route_test planned_host_budget_retains_prior_charge_and_aborts_at_frozen_cap` — raw log line 4497
- `surge-daemon::quota_recovery_route_test planned_no_open_park_survives_killed_host_and_periodic_wake_in_new_process` — raw log line 4632
- `surge-daemon::quota_recovery_route_test relative_post_429_source_change_remains_audit_only` — raw log line 4566
- `surge-daemon::quota_recovery_route_test relative_selected_source_change_blocks_provider_effect` — raw log line 4699
- `surge-daemon::quota_recovery_route_test relative_skipped_source_change_blocks_provider_effect` — raw log line 4765
- `surge-daemon::quota_recovery_route_test relative_sources_are_frozen_from_actual_launch_worktree_on_normal_start` — raw log line 4897
- `surge-daemon::quota_recovery_route_test selected_route_source_change_after_freeze_prevents_any_provider_effect` — raw log line 4831
- `surge-daemon::quota_recovery_route_test skipped_route_source_change_after_freeze_prevents_fallback_effect` — raw log line 4963
- `surge-daemon::quota_recovery_route_test skipped_route_source_change_prevents_stale_all_exhausted_park` — raw log line 5030
- `surge-daemon::quota_recovery_route_test task_owned_quota_dispatches_actual_a_then_b_in_the_same_workspace` — raw log line 5096
- `surge-persistence work_items::tests::live_start_preparation_refuses_generic_reservation` — raw log line 14745
- `surge-persistence work_items::tests::start_preparation_concurrent_exact_operation_has_one_attempt_and_receipt` — raw log line 14873
- `surge-persistence work_items::tests::start_preparation_dead_row_is_reclaimed_by_lifecycle_under_the_lock` — raw log line 14981
- `surge-persistence work_items::tests::start_preparation_does_not_hold_registry_lock_while_external_work_is_blocked` — raw log line 15041
- `surge-persistence work_items::tests::start_preparation_lifecycle_busy_discussion_drift_and_drop_release` — raw log line 15240
- `surge-persistence work_items::tests::start_preparation_snapshot_drift_never_creates_an_attempt` — raw log line 15301

### sharing-os32

- `surge-cli::bin/surge commands::bootstrap::tests::followup_failure_reaches_the_command_result` — raw log line 1367
- `surge-cli::bin/surge commands::bootstrap::tests::resumed_console_answers_only_the_fresh_pending_gate` — raw log line 1493
- `surge-cli::persistent_task_cli compiled_task_commands_create_start_replay_and_show_the_same_persistent_item` — raw log line 1433
- `surge-daemon bootstrap_supervisor::tests::cancellation_after_child_commit_cannot_admit_implementation` — raw log line 2639
- `surge-daemon bootstrap_supervisor::tests::incoming_bootstrap_cannot_bypass_older_durable_queue` — raw log line 2703
- `surge-daemon bootstrap_supervisor::tests::live_planning_gate_cancel_joins_and_never_launches_child` — raw log line 2765
- `surge-daemon bootstrap_supervisor::tests::real_planning_continues_to_child_with_one_admission_slot_and_isolation` — raw log line 2841
- `surge-daemon bootstrap_supervisor::tests::restart_after_child_commit_uses_committed_artifact_not_newer_parent_version` — raw log line 2903
- `surge-daemon bootstrap_supervisor::tests::restart_after_parent_completion_before_child_commit_reuses_parent_evidence` — raw log line 2967
- `surge-daemon::daemon_parity_test parity_terminal_minimal_agent_and_spike_local_vs_daemon` — raw log line 3250
- `surge-daemon::daemon_persisted_gate real_gate_request_and_resolution_arrive_before_terminal_and_slot_release` — raw log line 3125
- `surge-daemon::daemon_persisted_gate unreadable_run_stream_fails_waiter_but_another_run_stays_connected` — raw log line 3186
- `surge-daemon::daemon_resume_stream another_client_discovers_resumed_run_before_it_finishes` — raw log line 3374
- `surge-daemon::daemon_resume_stream fresh_connection_receives_fast_resume_before_followup_subscribe` — raw log line 3498
- `surge-daemon::daemon_resume_stream recovered_run_is_announced_before_completion` — raw log line 3436
- `surge-daemon::daemon_resume_stream rejected_resume_removes_preattached_forwarder_and_releases_admission` — raw log line 3312
- `surge-daemon::daemon_resume_stream same_connection_park_resume_replaces_finished_forwarder_and_delivers_gate` — raw log line 3560
- `surge-daemon::owned_flow_ipc locator_replay_survives_missing_source_and_changed_configuration` — raw log line 3669
- `surge-daemon::quota_recovery_route_test exhausted_candidates_park_and_scheduler_wakes_the_same_task` — raw log line 4231
- `surge-daemon::work_item_route_test accepted_gate_answer_receipt_survives_completion_and_restart` — raw log line 5231
- `surge-daemon::work_item_route_test accepted_gate_answer_survives_owner_death_before_outcome_delivery` — raw log line 5169
- `surge-daemon::work_item_route_test committed_gate_effects_survive_owner_death_before_route_without_repetition` — raw log line 5877
- `surge-daemon::work_item_route_test completed_gate_route_kind_must_match_accepted_graph` — raw log line 5815
- `surge-daemon::work_item_route_test daemon_gate_answer_requires_durable_acceptance_before_success` — raw log line 6327
- `surge-daemon::work_item_route_test disconnected_start_caller_does_not_drop_durable_attempt_or_active_run` — raw log line 6267
- `surge-daemon::work_item_route_test execution_owner_prevents_second_live_host_from_replaying_agent_turn` — raw log line 6387
- `surge-daemon::work_item_route_test expired_suspended_gate_answer_cannot_reset_original_deadline` — raw log line 6511
- `surge-daemon::work_item_route_test forged_suspended_human_gate_request_cannot_authorize_cold_continue` — raw log line 6447
- `surge-daemon::work_item_route_test killed_continue_ack_reconciles_nonterminal_control_before_waiting_for_operator` — raw log line 6889
- `surge-daemon::work_item_route_test killed_continue_owner_after_journal_ack_before_registry_ack_routes_once` — raw log line 7016
- `surge-daemon::work_item_route_test killed_continue_owner_after_route_before_journal_ack_routes_once` — raw log line 6643
- `surge-daemon::work_item_route_test killed_execution_owner_releases_os_lock_and_same_attempt_recovers` — raw log line 6769
- `surge-daemon::work_item_route_test live_tracking_error_with_conflicting_terminals_retains_task_ownership` — raw log line 6829
- `surge-daemon::work_item_route_test lost_reservation_empty_journal_and_provision_before_ack_retry_same_run_once` — raw log line 6956
- `surge-daemon::work_item_route_test manual_suspend_supersedes_reserved_continue_and_historical_replay_cannot_dispatch` — raw log line 7143
- `surge-daemon::work_item_route_test pure_human_gate_suspend_is_not_abort_and_reuses_original_decision` — raw log line 7205
- `surge-daemon::work_item_route_test revisited_human_gate_requires_new_occurrence_and_new_decision` — raw log line 7269
- `surge-daemon::work_item_route_test suspended_original_gate_answer_is_durable_before_continue` — raw log line 7396
- `surge-daemon::work_item_route_test task_created_over_daemon_survives_restart_and_runs_pinned_requirements` — raw log line 7460
- `surge-daemon::work_item_route_test task_provider_identity_is_durable_before_first_real_acp_prompt` — raw log line 7523
- `surge-orchestrator::archetypes_mock_test all_archetypes_complete_against_deterministic_mock_bridge` — raw log line 8333
- `surge-orchestrator::bootstrap_archetypes_test bootstrap_materializes_bug_fix_refactor_and_spike_archetypes` — raw log line 8400
- `surge-orchestrator::bootstrap_driver_e2e run_bootstrap_materializes_followup_graph` — raw log line 8462
- `surge-orchestrator::bootstrap_edit_cap_test repeated_description_edits_fail_after_configured_cap` — raw log line 8600
- `surge-orchestrator::bootstrap_linear_3_test bootstrap_linear_3_materializes_valid_followup_graph` — raw log line 8532
- `surge-orchestrator::bootstrap_multi_milestone_test bootstrap_multi_milestone_materializes_outer_and_inner_loops` — raw log line 8739
- `surge-orchestrator::bootstrap_validation_retry_test flow_generator_invalid_twice_then_valid_materializes_after_two_edits` — raw log line 8801
- `surge-orchestrator::elevation_blocked resolve_elevation_returns_run_not_found_for_unknown_run` — raw log line 8679
- `surge-orchestrator::engine_acp_permission mcp_session_ignores_injected_legacy_broadcast_authority` — raw log line 8875
- `surge-orchestrator::engine_agent_rotation_test a_rate_limited_stage_moves_to_the_fallback_agent_and_finishes` — raw log line 8954
- `surge-orchestrator::engine_agent_rotation_test a_rotation_onto_the_partner_agent_is_flagged` — raw log line 9014
- `surge-orchestrator::engine_agent_rotation_test with_every_fallback_exhausted_the_run_parks_and_wakes` — raw log line 9074
- `surge-orchestrator::engine_bootstrap_parent_artifacts_test a_roadmap_planner_alias_is_inherited_as_roadmap` — raw log line 9156
- `surge-orchestrator::engine_bootstrap_parent_artifacts_test start_run_with_bootstrap_parent_seeds_parent_artifacts` — raw log line 9216
- `surge-orchestrator::engine_budget_test run_aborts_when_token_budget_exceeded` — raw log line 9276
- `surge-orchestrator::engine_budget_test run_completes_within_budget` — raw log line 9336
- `surge-orchestrator::engine_budget_test warn_only_records_breach_but_completes` — raw log line 9396
- `surge-orchestrator::engine_capacity_park_test binding_failure_before_provider_open_preserves_exhaustion` — raw log line 9461
- `surge-orchestrator::engine_capacity_park_test configured_blind_backoff_reaches_the_park_decision_not_the_hardcoded_default` — raw log line 9521
- `surge-orchestrator::engine_capacity_park_test estimate_none_and_never_observed_does_not_block_dispatch` — raw log line 9581
- `surge-orchestrator::engine_capacity_park_test rate_limited_agent_parks_after_confirmed_forced_cleanup` — raw log line 9641
- `surge-orchestrator::engine_capacity_park_test rate_limited_agent_parks_instead_of_dispatching_a_second_node_on_the_same_runtime` — raw log line 9763
- `surge-orchestrator::engine_capacity_park_test registry_contention_does_not_delay_route_commit_behind_capacity_clear` — raw log line 9703
- `surge-orchestrator::engine_capacity_park_test resumed_run_driven_to_stage_failure_writes_to_the_configured_memory_store` — raw log line 9825
- `surge-orchestrator::engine_capacity_park_test resuming_a_parked_run_makes_one_real_attempt_and_clears_the_stale_row` — raw log line 9886
- `surge-orchestrator::engine_capacity_park_test resuming_a_parked_run_re_arms_its_frozen_token_budget` — raw log line 9956
- `surge-orchestrator::engine_capacity_park_test surge_toml_blind_backoff_reaches_the_park_decision_through_surge_config_discover` — raw log line 10020
- `surge-orchestrator::engine_concurrent_unit three_concurrent_runs_complete_independently` — raw log line 10081
- `surge-orchestrator::engine_escalation_gate_test a_restarted_host_reissues_the_escalation_gate_and_honours_stop` — raw log line 10203
- `surge-orchestrator::engine_escalation_gate_test accept_as_is_continues_on_the_success_path_and_records_a_human_acceptance` — raw log line 10141
- `surge-orchestrator::engine_escalation_gate_test an_exhausted_task_is_split_and_its_subtasks_run_in_its_place` — raw log line 10272
- `surge-orchestrator::engine_escalation_gate_test exhausted_loop_asks_then_retries_with_findings_then_stops` — raw log line 10337
- `surge-orchestrator::engine_escalation_gate_test revise_requirement_reruns_the_stage_against_the_revision` — raw log line 10522
- `surge-orchestrator::engine_gate_cancellation stop_pending_gate_is_durably_aborted_without_a_decision` — raw log line 10402
- `surge-orchestrator::engine_gate_cancellation stop_racing_a_real_decision_preserves_a_complete_event_sequence` — raw log line 10462
- `surge-orchestrator::engine_gate_identity old_card_cannot_resolve_a_new_visit_to_the_same_gate` — raw log line 10707
- `surge-orchestrator::engine_gate_identity reopening_after_owner_loss_reissues_an_unanswered_gate_with_fresh_identity` — raw log line 10772
- `surge-orchestrator::engine_guard_spill_harness_test configured_repeat_threshold_is_honored_not_the_default` — raw log line 10644
- `surge-orchestrator::engine_guard_spill_harness_test configured_spill_cap_is_honored_and_lands_in_the_runs_own_artifact_store` — raw log line 10584
- `surge-orchestrator::engine_guard_spill_harness_test node_wall_clock_deadline_trips_without_any_tool_call` — raw log line 10858
- `surge-orchestrator::engine_initial_prompt_test start_run_seeds_user_prompt_artifact_from_initial_prompt` — raw log line 10924
- `surge-orchestrator::engine_initial_prompt_test start_run_with_empty_prompt_skips_seeding` — raw log line 10988
- `surge-orchestrator::engine_loop_protection_test an_idle_attempt_counts_as_failed_and_retries` — raw log line 11238
- `surge-orchestrator::engine_loop_protection_test repeated_identical_calls_end_the_attempt_and_retry` — raw log line 11049
- `surge-orchestrator::engine_loop_protection_test the_tool_call_cap_ends_the_attempt_and_retries` — raw log line 11111
- `surge-orchestrator::engine_loop_protection_test wall_clock_trips_climb_the_ladder_to_the_human_gate` — raw log line 11177
- `surge-orchestrator::engine_m6_loop_max_traversals loop_max_iterations_2_runs_at_most_2_body_executions` — raw log line 11367
- `surge-orchestrator::engine_m6_loop_retry abort_policy_stops_at_first_failed_item` — raw log line 11305
- `surge-orchestrator::engine_m6_loop_retry every_item_receives_its_own_retry_budget` — raw log line 11427
- `surge-orchestrator::engine_m6_loop_retry loop_body_retries_up_to_max_then_succeeds` — raw log line 11489
- `surge-orchestrator::engine_m6_loop_retry loop_failure_retries_exactly_to_cap_then_fails` — raw log line 11551
- `surge-orchestrator::engine_m6_loop_retry zero_retry_budget_still_attempts_the_item_once` — raw log line 11618
- `surge-orchestrator::engine_m6_loop_skip_failure loop_body_failure_with_skip_policy_continues_loop` — raw log line 11753
- `surge-orchestrator::engine_m6_loop_skip_failure loop_failure_with_skip_continues_through_all_items` — raw log line 11691
- `surge-orchestrator::engine_m6_notify_webhook notify_webhook_posts_body_containing_run_id` — raw log line 11880
- `surge-orchestrator::engine_m6_static_loop approved_run_artifact_drives_loop_without_planner` — raw log line 11820
- `surge-orchestrator::engine_m6_static_loop empty_loop_completion_has_no_pushed_frame_and_remains_trusted` — raw log line 11940
- `surge-orchestrator::engine_m6_static_loop fork_inside_loop_keeps_iteration_position_and_remaining_items` — raw log line 12140
- `surge-orchestrator::engine_m6_static_loop nested_subgraph_loop_retains_counter_and_declared_synthetic_route` — raw log line 12073
- `surge-orchestrator::engine_m6_static_loop three_iteration_static_loop_completes` — raw log line 12007
- `surge-orchestrator::engine_m6_subgraph_simple subgraph_emits_entered_and_exited_then_completes` — raw log line 12200
- `surge-orchestrator::engine_profile_catalog_seed_test start_run_seeds_nothing_and_still_starts_without_profile_registry` — raw log line 12271
- `surge-orchestrator::engine_profile_catalog_seed_test start_run_seeds_profile_catalog_artifact_when_registry_wired` — raw log line 12331
- `surge-orchestrator::engine_project_context_seed_test start_run_seeds_configured_run_artifacts` — raw log line 12396
- `surge-orchestrator::engine_project_context_seed_test start_run_seeds_project_context_artifact` — raw log line 12456
- `surge-orchestrator::engine_skill_binding_test approved_skill_completion_does_not_leave_a_human_gate_recovery_record` — raw log line 12521
- `surge-orchestrator::engine_skill_binding_test approved_skill_gate_does_not_leak_into_legitimate_stage_revisit` — raw log line 12583
- `surge-orchestrator::engine_skill_binding_test pinned_skill_binds_and_instructions_reach_the_agent_prompt` — raw log line 12650
- `surge-orchestrator::engine_skill_binding_test plugin_packaged_skill_binds_via_dot_claude_plugins_root` — raw log line 12710
- `surge-orchestrator::engine_skill_binding_test unpinned_skill_unanswered_rejects_before_any_session_opens` — raw log line 12770
- `surge-orchestrator::engine_snapshot_unit resume_after_completion_is_idempotent` — raw log line 12895
- `surge-orchestrator::engine_snapshot_unit single_terminal_run_has_no_stage_boundary_snapshots` — raw log line 12835
- `surge-orchestrator::engine_snapshot_unit three_node_branch_run_writes_two_snapshots` — raw log line 12955
- `surge-orchestrator::engine_stage_commit_test rejected_stage_snapshot_rolls_back_routing_events` — raw log line 13016
- `surge-orchestrator::engine_start_run_smoke start_run_smoke_completes_terminal_node` — raw log line 13095
- `surge-orchestrator::memory_writeback_test a_verified_claim_covering_the_same_node_is_left_untouched` — raw log line 13527
- `surge-orchestrator::memory_writeback_test clean_run_writes_no_memory_claim` — raw log line 13587
- `surge-orchestrator::memory_writeback_test failing_run_records_a_claim_visible_to_the_audit` — raw log line 13647
- `surge-orchestrator::memory_writeback_test records_the_first_rejecting_hook_not_the_last_one` — raw log line 13720
- `surge-orchestrator::memory_writeback_test second_failure_of_the_same_node_updates_the_claim_in_place` — raw log line 13780
- `surge-orchestrator::on_error_suppress_test on_error_suppresses_failure_into_declared_outcome` — raw log line 13944
- `surge-orchestrator::on_error_suppress_test undeclared_suppression_falls_through_to_stage_failed` — raw log line 14006
- `surge-orchestrator::on_error_suppress_test without_on_error_hook_crash_records_stage_failed` — raw log line 13847
- `surge-orchestrator::verification_revision_test on_error_shell_mutation_fences_old_proof_even_after_restore` — raw log line 14115
- `surge-orchestrator::verification_revision_test superseded_accepted_requirements_cannot_reuse_a_still_current_historical_run_proof` — raw log line 14175
- `surge-telegram::production_callbacks accepted_callback_with_failed_card_close_never_says_decision_failed` — raw log line 15513
- `surge-telegram::production_callbacks accepted_edit_with_failed_card_close_never_says_decision_failed` — raw log line 15575
- `surge-telegram::production_callbacks bot_api_failure_does_not_roll_back_accepted_engine_decision` — raw log line 16266
- `surge-telegram::production_callbacks force_reply_correlation_survives_reopened_storage_and_routes` — raw log line 15637
- `surge-telegram::production_callbacks force_reply_send_failure_grants_no_reply_authority` — raw log line 16010
- `surge-telegram::production_callbacks force_reply_sent_but_not_persisted_grants_no_reply_authority` — raw log line 15700
- `surge-telegram::production_callbacks forced_reply_requires_exact_chat_prompt_and_card` — raw log line 15762
- `surge-telegram::production_callbacks old_open_card_cannot_approve_new_gate_and_reports_stale` — raw log line 15824
- `surge-telegram::production_callbacks production_approve_acknowledges_and_resolves_exact_gate` — raw log line 15886
- `surge-telegram::production_callbacks recovery_closes_exact_resolved_card_while_next_gate_stays_open` — raw log line 15948
- `surge-telegram::production_callbacks reject_terminates_real_gate_after_foreign_paired_chat_is_denied` — raw log line 16072
- `surge-telegram::production_callbacks resolver_failure_still_acknowledges_and_never_claims_success` — raw log line 16134
- `surge-telegram::production_callbacks sole_update_stream_routes_inbox_and_correlated_feedback` — raw log line 16196

### suspension-attention-mismatch

- `surge-daemon::work_item_route_test task_continue_without_provider_restore_capability_preserves_attention` — raw log line 7330
- `surge-daemon::work_item_route_test task_suspend_continue_uses_saved_provider_on_real_acp_wire` — raw log line 7591

