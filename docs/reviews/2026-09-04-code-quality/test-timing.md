# Per-test timing

Measured on 2026-09-04 at commit 19312ba (the hash the tree carries after the history rewrite; it was 3046327 when measured): `cargo test --workspace -- --report-time --test-threads=1`, debug profile, Windows 11 host, whatever adapter each test selects (the golden suite forces the software adapter). Wall time including compilation from a cold target directory: 8m49s. `cargo clippy --all-targets`: 0 warnings. Note that libtest runs tests in name order, so the alphabetically first test of a target that shares a device or engine through a `LazyLock` pays that initialization.


## Totals per target (sum of per-test times, count, count over 1 s, count over 0.1 s)

tests/engine.rs              sum=  161.62s  n=  78  >1s= 76  >0.1s= 77
tests/soak.rs                sum=   65.58s  n=   1  >1s=  1  >0.1s=  1
unittests                    sum=    4.17s  n=1215  >1s=  0  >0.1s=  9
tests/golden.rs              sum=    4.00s  n=  20  >1s=  1  >0.1s=  4
tests/render_pipeline.rs     sum=    2.08s  n=  21  >1s=  1  >0.1s=  2
tests/shading.rs             sum=    1.55s  n=  12  >1s=  0  >0.1s=  2
tests/slint_ui.rs            sum=    0.61s  n=  45  >1s=  0  >0.1s=  1

## Every test over 0.1 s (seconds, name), grouped by target

tests/engine.rs               20.336  lowering_the_resolution_lowers_the_process_footprint
tests/engine.rs                7.581  the_panorama_follows_the_texture_resolution_cap
tests/engine.rs                4.998  the_real_panorama_has_the_galactic_plane_where_the_plane_is
tests/engine.rs                3.718  one_monitor_is_one_image_at_its_own_size_in_every_mode
tests/engine.rs                3.600  the_sunrise_band_arrives_with_the_suns_image
tests/engine.rs                3.263  a_switch_keeps_the_old_cloud_texture_until_the_new_one_lands
tests/engine.rs                3.155  a_dayside_cloud_is_brighter_than_a_night_side_one_in_every_mode
tests/engine.rs                2.838  a_sun_grazing_the_limb_turns_the_glare_warm
tests/engine.rs                2.784  the_physical_exposure_has_no_peak
tests/engine.rs                2.651  a_layout_that_changed_after_a_refusal_is_announced_and_not_published
tests/engine.rs                2.602  a_switched_off_moon_and_a_missing_texture_draw_the_same_frame
tests/engine.rs                2.599  a_taller_canvas_still_puts_the_globe_where_the_anchor_had_it
tests/engine.rs                2.578  one_screen_paints_the_anchor_and_leaves_the_others_alone
tests/engine.rs                2.526  the_anchors_crop_of_a_span_is_the_picture_it_would_have_had_alone
tests/engine.rs                2.517  a_pan_past_the_frame_corner_still_draws_the_sun
tests/engine.rs                2.516  the_glare_peaks_as_the_disk_clears_the_horizon
tests/engine.rs                2.492  a_hint_during_the_settle_moves_the_deadline_it_found
tests/engine.rs                2.435  every_screen_renders_one_image_per_distinct_size
tests/engine.rs                2.299  a_switch_to_the_current_resolution_does_nothing
tests/engine.rs                2.116  a_night_side_cloud_is_brighter_than_the_land_under_it
tests/engine.rs                2.083  a_stale_arrival_produces_no_frame_at_all
tests/engine.rs                2.019  a_hint_about_a_layout_that_did_not_change_is_one_query_and_nothing_else
tests/engine.rs                2.011  disabling_the_preview_stops_frames_without_stopping_the_engine
tests/engine.rs                1.992  unchanged_parameters_do_not_produce_another_frame
tests/engine.rs                1.804  a_wallpaper_update_during_a_reload_waits_for_the_textures
tests/engine.rs                1.787  a_switch_while_the_first_load_is_running_still_converges
tests/engine.rs                1.769  re_enabling_the_preview_resends_the_current_frame_unchanged
tests/engine.rs                1.682  a_layout_that_changed_with_nothing_on_the_desk_is_announced_and_not_published
tests/engine.rs                1.632  switching_texture_mode_produces_a_new_frame
tests/engine.rs                1.631  an_opacity_at_zero_switches_off_only_its_own_hemisphere
tests/engine.rs                1.627  a_switch_down_leaves_no_texture_at_the_old_width
tests/engine.rs                1.623  the_panorama_tracks_the_sky_field_of_view_and_the_globe_does_not
tests/engine.rs                1.619  a_stored_anchor_that_is_gone_falls_back_and_reports_it
tests/engine.rs                1.614  the_night_opacity_reaches_full_cover
tests/engine.rs                1.612  every_hint_of_one_burst_collapses_into_a_single_query
tests/engine.rs                1.610  a_moon_at_the_view_antipode_draws_nothing
tests/engine.rs                1.583  the_panorama_puts_a_landmark_where_the_star_path_puts_the_same_direction
tests/engine.rs                1.573  the_measured_and_computed_texture_totals_agree
tests/engine.rs                1.568  preview_size_changes_are_quantized_and_applied
tests/engine.rs                1.557  a_switch_back_to_a_cached_variant_shows_it_again
tests/engine.rs                1.536  changed_parameters_produce_a_new_frame
tests/engine.rs                1.510  the_wrap_column_is_not_a_band_of_the_coarsest_mip
tests/engine.rs                1.507  an_unsupported_sample_count_arriving_later_still_renders
tests/engine.rs                1.479  a_sink_that_cannot_publish_is_never_asked_to_render
tests/engine.rs                1.477  a_resolution_switch_reloads_the_textures_in_both_directions
tests/engine.rs                1.438  the_lit_fraction_tracks_the_ephemeris_at_three_phases
tests/engine.rs                1.428  a_moon_over_the_sun_fades_the_glare_around_it
tests/engine.rs                1.421  a_layout_that_changed_under_a_published_wallpaper_is_published_again
tests/engine.rs                1.390  engine_renders_a_first_preview_frame
tests/engine.rs                1.372  earthshine_lifts_the_unlit_face_only
tests/engine.rs                1.370  a_new_display_plan_changes_the_next_publish
tests/engine.rs                1.368  the_cloud_variant_follows_the_texture_resolution
tests/engine.rs                1.352  a_panorama_fills_the_sky_and_zero_intensity_empties_it
tests/engine.rs                1.339  switches_in_quick_succession_end_on_the_last_one
tests/engine.rs                1.331  a_stale_decode_must_not_replace_the_texture_that_superseded_it
tests/engine.rs                1.329  the_report_names_the_textures_the_renderer_owns
tests/engine.rs                1.314  render_to_file_writes_a_png_at_the_requested_size
tests/engine.rs                1.297  a_moon_on_the_night_sky_only_adds_light
tests/engine.rs                1.288  enabling_the_preview_before_any_frame_exists_still_delivers_one
tests/engine.rs                1.273  zero_sun_glow_takes_the_sun_out_of_the_frame
tests/engine.rs                1.265  an_unsupported_sample_count_still_renders
tests/engine.rs                1.260  the_lit_limb_faces_the_sun
tests/engine.rs                1.256  across_screens_renders_one_canvas_and_cuts_it
tests/engine.rs                1.254  wider_sky_fov_reveals_more_catalog_directions
tests/engine.rs                1.250  a_portrait_screen_takes_the_contained_lens_instead_of_the_old_one
tests/engine.rs                1.247  a_sliver_of_sun_over_the_limb_still_glares
tests/engine.rs                1.244  a_sun_behind_the_painted_globe_paints_nothing
tests/engine.rs                1.226  the_moon_texture_lands_in_its_own_slot_without_delaying_readiness
tests/engine.rs                1.221  a_pan_slides_the_composite_without_shearing_it
tests/engine.rs                1.207  larger_star_size_expands_crisp_cores_when_glow_is_disabled
tests/engine.rs                1.202  an_enlarged_disk_reaching_the_frame_corner_is_drawn
tests/engine.rs                1.189  textures_ready_fires_for_the_procedural_grid
tests/engine.rs                1.189  wallpaper_now_publishes_one_frame_at_the_sink_size
tests/engine.rs                1.187  the_allocator_section_reports_reserved_at_least_as_large_as_allocated
tests/engine.rs                1.177  zero_star_intensity_leaves_catalog_pixels_at_the_clear_color
tests/engine.rs                1.130  render_to_file_works_with_the_preview_disabled
tests/engine.rs                0.299  no_bright_star_is_baked_into_the_real_panorama
tests/golden.rs                2.141  contact_sheet_of_every_preset
tests/golden.rs                0.750  every_golden_case_is_distinguishable
tests/golden.rs                0.352  golden_cloud_terminator_close_up
tests/golden.rs                0.156  golden_panorama_at_a_narrow_sky
tests/render_pipeline.rs       1.414  cloud_pipeline_renders_with_alpha
tests/render_pipeline.rs       0.523  the_shader_and_the_cpu_agree_on_the_three_shared_rules
tests/shading.rs               0.897  diffuse_disabled_matches_pure_blend
tests/shading.rs               0.649  software_adapter_produces_correct_results
tests/slint_ui.rs              0.139  test_the_adapter_line_never_wraps
tests/soak.rs                 65.582  fourteen_simulated_days_of_clouds_and_exports_stay_bounded
unittests                      0.933  assets::texture_loader::tests::downsample_output_size_invariant
unittests                      0.542  guest::script_syntax::tests::every_shell_script_the_linux_build_runs_parses
unittests                      0.404  guest::script_syntax::tests::every_generated_powershell_script_parses
unittests                      0.296  runner::windows_boundary_tests::a_powershell_script_on_stdin_produces_output_here
unittests                      0.275  guest::script_syntax::tests::the_checker_would_notice_a_broken_script
unittests                      0.230  wallpaper::tests::every_monitor_is_enumerated_with_a_rectangle_and_one_of_them_is_primary
unittests                      0.162  scene::sky::tests::the_sub_earth_point_stays_inside_the_libration_bounds
unittests                      0.133  scene::sky::tests::the_moon_orbits_between_fifty_five_and_sixty_four_earth_radii
unittests                      0.108  scene::sun_occlusion::tests::a_squashed_disk_against_the_limb_is_a_round_one_against_a_moved_limb
