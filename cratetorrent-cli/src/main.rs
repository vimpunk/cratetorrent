use {
    clap::{App, Arg},
    cratetorrent::metainfo::*,
    std::{fs, path::PathBuf},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // set up cli args
    let matches = App::new("Cratetorrent CLI")
        .version("1.0.0-alpha.1")
        .arg(
            Arg::with_name("seeds")
                .short("s")
                .long("seeds")
                .value_name("SOCKET_ADDRS")
                .help(
                    "A comma separated list of <ip>:<port> pairs of the seeds",
                )
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
        .arg(
            Arg::with_name("download-dir")
                .short("d")
                .long("download-dir")
                .value_name("PATH")
                .help("The path of the folder where to download file")
                .takes_value(true),
        )
        .get_matches();

    // parse cli args
    let seeds = matches
        .value_of("seeds")
        .ok_or_else(|| "--seeds must be set")?
        .split(",")
        .filter_map(|s| s.parse().ok())
        .collect();
    let metainfo_path = matches
        .value_of("metainfo")
        .ok_or_else(|| "--seed must be set")?;
    let download_dir: PathBuf = matches
        .value_of("download-dir")
        .ok_or_else(|| "--download-dir must be set")?
        .into();

    // read in torrent metainfo
    let metainfo = fs::read(metainfo_path)?;
    let metainfo = Metainfo::from_bytes(&metainfo)?;
    println!("metainfo: {:?}", metainfo);
    println!("piece count: {}", metainfo.piece_count());
    println!("info hash: {}", hex::encode(&metainfo.info_hash));

    // arbitrary client id for now
    const CLIENT_ID: &str = "cbt-2020-03-03-00000";
    let mut client_id = [0; 20];
    client_id.copy_from_slice(CLIENT_ID.as_bytes());

    cratetorrent::engine::run_torrent(
        client_id,
        download_dir,
        metainfo,
        seeds,
    )?;

    Ok(())
}
