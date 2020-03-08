use clap::{App, Arg};
use cratetorrent::metainfo::*;
use cratetorrent::run_torrent;
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // set up cli args
    let matches = App::new("Cratetorrent CLI")
        .version("1.0.0-alpha.1")
        .arg(
            Arg::with_name("seed")
                .short("s")
                .long("seed")
                .value_name("SOCKET_ADDR")
                .help("The <ip>:<port> pair of the BitTorrent seed")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("metainfo")
                .short("i")
                .long("metainfo")
                .value_name("PATH")
                .help("The path to the torrent metainfo file")
                .takes_value(true),
        )
        .get_matches();

    // parse cli args
    let seed = matches
        .value_of("seed")
        .ok_or_else(|| "--seed must be set")?
        .parse()?;
    let metainfo_path = matches
        .value_of("metainfo")
        .ok_or_else(|| "--seed must be set")?;

    // read in torrent metainfo
    let metainfo = fs::read(metainfo_path)?;
    let metainfo = Metainfo::from_bytes(&metainfo)?;
    println!("metainfo: {:?}", metainfo);
    let info_hash = metainfo.create_info_hash()?;
    println!("info hash: {}", hex::encode(&info_hash));

    // arbitrary client id for now
    const CLIENT_ID: &str = "cbt-2020-03-03-00000";
    let mut client_id = [0; 20];
    client_id.copy_from_slice(CLIENT_ID.as_bytes());

    run_torrent(client_id, metainfo, seed)?;

    Ok(())
}
