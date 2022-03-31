mod utils;
mod schema;
mod delegations;
use std::io::BufReader;
use std::fs::File;
use winldap::connection::{LdapConnection, LdapCredentials};
use windows::Win32::Networking::Ldap::LDAP_PORT;
use clap::{App, Arg};
use serde_json;
use crate::schema::Schema;
use crate::delegations::{Delegation, get_explicit_delegations, get_schema_delegations};
use crate::utils::{get_forest_sid, get_adminsdholder_sd};

fn main() {
    let default_port = format!("{}", LDAP_PORT);
    let app = App::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .about(env!("CARGO_PKG_DESCRIPTION"))
        .arg(
            Arg::new("server")
                .help("(explicit server) LDAP server hostname or IP")
                .long("server")
                .short('s')
                .number_of_values(1)
        )
        .arg(
            Arg::new("port")
                .help("(explicit server) LDAP port")
                .long("port")
                .number_of_values(1)
                .default_value(&default_port)
        )
        .arg(
            Arg::new("domain")
                .help("(explicit credentials) Logon domain name")
                .long("domain")
                .short('d')
                .number_of_values(1)
                .requires_all(&["username", "password"])
        )
        .arg(
            Arg::new("username")
                .help("(explicit credentials) Logon user name")
                .long("user")
                .short('u')
                .number_of_values(1)
                .requires_all(&["domain","password"])
        )
        .arg(
            Arg::new("password")
                .help("(explicit credentials) Logon Password")
                .long("password")
                .short('p')
                .number_of_values(1)
                .requires_all(&["domain","username"])
        )
        .arg(
            Arg::new("deleg_file")
                .value_name("FILE")
                .multiple_occurrences(true)
                .help("json file with delegation templates and/or actual delegations")
                .number_of_values(1)
        );

    let args = app.get_matches();

    let base_delegations: Vec<Delegation> = {
        let mut res = vec![];
        let input_filepaths: Vec<&str> = args.values_of("deleg_file").unwrap().collect();
        for input_filepath in &input_filepaths {
            let file = match File::open(input_filepath) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!(" [!] Unable to open file {} : {}", input_filepath, e);
                    std::process::exit(1);
                }
            };
            let reader = BufReader::new(file);
            match serde_json::from_reader(reader) {
                Ok(mut v) => res.append(&mut v),
                Err(e) => {
                    eprintln!(" [!] Unable to parse file {} : {}", input_filepath, e);
                    std::process::exit(1);
                }
            }
        }
        res
    };

    let server= args.value_of("server");
    let port = args.value_of("port").expect("no port set");
    let port = match port.parse::<u16>() {
        Ok(n) if n > 0 => n,
        _ => {
            eprintln!("Unable to parse \"{}\" as TCP port", port);
            std::process::exit(1);
        }
    };
    let credentials = match (args.value_of("domain"),
                             args.value_of("username"),
                             args.value_of("password")) {
        (Some(d), Some(u), Some(p)) => {
            Some(LdapCredentials {
                domain: d,
                username: u,
                password: p,
            })
        },
        _ => None,
    };

    let conn = match LdapConnection::new(server, port, credentials.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Unable to connect to \"{}:{}\" : {}", server.unwrap_or("default"), port, e);
            std::process::exit(1);
        }
    };

    let forest_sid = match get_forest_sid(&conn) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Unable to fetch forest SID: {}", e);
            std::process::exit(1);
        }
    };

    let adminsdholder_sd = match get_adminsdholder_sd(&conn) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Unable to fetch AdminSDHolder security descriptor: {}", e);
            std::process::exit(1);
        }
    };

    let schema = match Schema::query(&conn) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Unable to fetch required information from schema: {}", e);
            std::process::exit(1);
        }
    };

    let mut delegations = vec![];
    
    delegations.append(&mut get_schema_delegations(&schema, &forest_sid));

    for naming_context in conn.get_naming_contexts() {
        println!("Fetching security descriptors of naming context {}", naming_context);
        match get_explicit_delegations(&conn, naming_context, &forest_sid, &schema, &adminsdholder_sd) {
            Ok(mut h) => delegations.append(&mut h),
            Err(e) => {
                eprintln!(" [!] Error when fetching security descriptors of naming context {} : {}", naming_context, e);
                std::process::exit(1);
            },
        };
    }

    if let Err(e) = conn.destroy() {
        eprintln!("Error when closing connection to \"{}:{}\" : {}", server.unwrap_or("default"), port, e);
        std::process::exit(1);
    }

    for deleg in &delegations {
        let mut found = false;
        for base_deleg in &base_delegations {
            if deleg.is_instance_of(&base_deleg) {
                found = true;
                break;
            }
        }
        if found {
            continue;
        }
        println!("\n{}", serde_json::to_string_pretty(deleg).unwrap());
    }
}
