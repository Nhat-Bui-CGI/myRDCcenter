//! Print what an .rdm file would import: `cargo run -p myrdc-core --example rdm_dump -- file.rdm`
fn main() {
    let path = std::env::args().nth(1).expect("usage: rdm_dump <file.rdm>");
    match myrdc_core::import_rdm::import_rdm(std::path::Path::new(&path)) {
        Ok(imp) => {
            println!("groups: {:?}", imp.groups);
            for c in &imp.connections {
                println!("{} | group={} host={} port={} user={} domain={} admin={}", c.name, c.group, c.host, c.port, c.username, c.domain, c.admin_session);
            }
            println!("passwords skipped: {}, other entries skipped: {}", imp.passwords_skipped, imp.other_skipped);
        }
        Err(e) => eprintln!("{e}"),
    }
}
