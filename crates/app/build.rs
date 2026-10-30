/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

fn main() {
    embed_resource::compile("framelet.rc", embed_resource::NONE).manifest_required().unwrap();
    println!("cargo:rerun-if-changed=framelet.manifest");
    println!("cargo:rerun-if-changed=../../assets/framelet.ico");
    println!("cargo:rerun-if-changed=../../assets/framelet-rec.ico");
}
