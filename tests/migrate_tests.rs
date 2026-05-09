use ekapkgs_update::commands::migrate::apply_run_unit_tests_migration;

#[test]
fn test_migrate_cmocka() {
    let input = include_str!("migrate/cmocka.test.in.nix");
    let expected_output = include_str!("migrate/cmocka.test.out.nix");

    let result = apply_run_unit_tests_migration(input).unwrap();
    assert_eq!(result, expected_output);
}
