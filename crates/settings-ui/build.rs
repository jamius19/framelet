/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

fn main() {
    slint_build::compile("ui/app.slint").expect("failed to compile Slint UI");
    embed_resource::compile("framelet-settings.rc", embed_resource::NONE).manifest_required().unwrap();
    println!("cargo:rerun-if-changed=../../assets/framelet.ico");
}
