//! The engine of the Shaiya SVMap Editor without any GUI: documents with undo, pairing of server and client records, the file and
//! cross-file checks, the spawn report, the bulk planner, the Map.ini model, the new-map engine and the staging and install code,
//! plus [`batch`], the entry points of `shaiya svmap`. No printing, no environment reads, no process starts.

pub mod assets;
pub mod batch;
pub mod bulk;
pub mod client_files;
pub mod client_rules;
pub mod compare;
pub mod cross_rules;
pub mod doc;
pub mod edit;
pub mod episode;
pub mod export;
pub mod fixes;
pub mod folderdiff;
pub mod generate;
pub mod heat;
pub mod ini_doc;
pub mod keys;
pub mod mapini;
pub mod math3d;
pub mod measure;
pub mod names;
pub mod newmap;
pub mod pair;
pub mod place;
pub mod problems;
pub mod scatter;
pub mod search;
pub mod server_data;
pub mod share;
pub mod spawns;
pub mod stamp;
pub mod sync;
pub mod terrain;
pub mod world;

#[cfg(any(test, feature = "testing"))]
pub mod testutil;
