//! What the memory and the code index both use.
//!
//! Each tool of Embornal keeps one SQLite file and answers a question with two
//! indexes. The parts of that which do not know whether they hold a fact or a
//! function live here, so that neither tool reaches inside the other.
//!
//! - [`score`]: the arithmetic that puts two indexes on one scale.
//! - [`sqlite`]: how a file opens and how its schema walks forward.

pub mod score;
pub mod sqlite;
