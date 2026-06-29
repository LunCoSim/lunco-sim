//! Backend-agnostic networking core — prototype + test bed.
//!
//! Pure-logic implementations of the parts of the sync architecture that do NOT
//! depend on the networking backend (lightyear/replicon), so they can be tested
//! cheaply and exhaustively *before* committing to a backend or paying a heavy
//! build. (The original `../NETWORKING_TEST_PLAN.md` — what this covers vs. what
//! becomes a headless integration test once the backend lands — is in git history.)

pub mod identity;
pub mod rebase;
pub mod sync_class;
