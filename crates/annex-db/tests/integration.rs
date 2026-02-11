use annex_db::{create_pool, run_migrations};

#[test]
fn db_initialization_works() {
    let pool = create_pool(":memory:").expect("failed to create pool");
    let conn = pool.get().expect("failed to get connection");
    let applied = run_migrations(&conn).expect("failed to run migrations");
    assert_eq!(applied, 1);

    // Verify table count (excluding sqlite_sequence and internal tables)
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .expect("failed to prepare table count query");
    let tables: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .expect("failed to execute table count query")
        .map(|r| r.expect("failed to read table name"))
        .collect();

    assert_eq!(tables.len(), 1, "expected only 1 table");
    assert_eq!(tables[0], "_annex_migrations");
}
