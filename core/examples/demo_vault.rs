//! Make a demo vault with sample connections, for screenshots and testing.
//!
//! cargo run -p myrdc-core --example demo_vault -- <path>\demo.rdsc
//!
//! The vault password is "demo-password". On Windows and macOS the vault is also
//! remembered on this computer, so the apps open it without asking.
//! Start an app with MYRDCCENTER_VAULT=<path>\demo.rdsc to use it.

use myrdc_core::{Connection, Vault};

fn main() {
    let path = std::env::args().nth(1).expect("usage: demo_vault <path to .rdsc>");
    let path = std::path::PathBuf::from(path);
    let _ = std::fs::remove_file(&path);
    let mut v = Vault::create(&path, "demo-password").expect("create vault");

    let items = [
        ("File server", "Work/Servers", "fs01.contoso.local", "admin"),
        ("SQL server", "Work/Servers", "sql01.contoso.local", "dba"),
        ("Build agent", "Work/Servers", "build02.contoso.local", "builder"),
        ("Office PC", "Work", "pc-0142.contoso.local", "nhat"),
        ("ERP terminal", "Customers/Fabrikam", "erp.fabrikam.example", "support"),
        ("Web server", "Customers/Fabrikam", "web01.fabrikam.example", "support"),
        ("Jump host", "Customers/Northwind", "jump.northwind.example", "ops"),
        ("Test VM", "Lab", "192.168.10.20", "lab"),
        ("Home PC", "", "home-pc.local", "me"),
    ];
    for (name, group, host, user) in items {
        let mut c = Connection::new(name, group);
        c.host = host.into();
        c.username = user.into();
        if group.starts_with("Work") {
            c.domain = "CONTOSO".into();
        }
        if name == "SQL server" {
            c.gateway = "gw.contoso.com".into();
            c.notes = "Maintenance window: Sunday 02:00-04:00".into();
        }
        v.data.connections.push(c);
    }
    for g in ["Work/Servers", "Customers/Fabrikam", "Customers/Northwind", "Lab"] {
        v.data.add_group(g);
    }
    v.save().expect("save vault");
    if let Err(e) = v.set_remembered(true) {
        eprintln!("Could not remember the vault on this computer: {e}");
    }
    println!("Demo vault: {}", path.display());
}
