use clap::{App, Arg, SubCommand};
use cratetorrent::metainfo::*;
use cratetorrent::*;
use std::fs;
use std::net::SocketAddr;
use tokio::runtime::Runtime;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

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
    let seed = matches.value_of("seed").unwrap().parse().unwrap();
    let metainfo_path = matches.value_of("metainfo").unwrap();

    let metainfo = fs::read(metainfo_path).unwrap();
    let metainfo = Metainfo::from_bytes(&metainfo).unwrap();
    println!("metainfo: {:?}", metainfo);
    let info_hash = metainfo.create_info_hash().unwrap();
    println!("info hash: {}", hex::encode(&info_hash));

    // arbitrary peer id for now
    const PEER_ID: &str = "cbt-2020-03-03-00000";
    let mut client_id = [0; 20];
    client_id.copy_from_slice(PEER_ID.as_bytes());

    let mut rt = Runtime::new()?;
    let torrent_fut = run_torrent(client_id, metainfo, seed);
    rt.block_on(torrent_fut)?;

    Ok(())
}
