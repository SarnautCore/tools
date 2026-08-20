use std::io::Result;
use std::path::Path;

fn main() -> Result<()> {
    let proto = Path::new("proto/sarnaut/content/v1/content.proto");
    println!("cargo:rerun-if-changed={}", proto.display());

    let mut config = prost_build::Config::new();
    // BTreeMap keeps `extra` iteration order stable, which pack_id depends on.
    config.btree_map(["."]);
    config.compile_protos(&[proto], &[Path::new("proto")])
}
