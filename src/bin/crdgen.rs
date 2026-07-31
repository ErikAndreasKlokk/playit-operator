//! Prints the `PlayitTunnel` CustomResourceDefinition as YAML to stdout.
//!
//! Regenerate the shipped manifest with:
//! `cargo run --bin crdgen > deploy/crd.yaml`

use kube::CustomResourceExt;
use playit_operator::crd::PlayitTunnel;

fn main() {
    let crd = PlayitTunnel::crd();
    print!("{}", serde_yaml::to_string(&crd).expect("serialize CRD"));
}
