use url::Url;
use web_crawler_engine_v3::sqlite_cache::CacheTag;

pub fn randomize_parsed_seed_order(
    parsed: &mut Vec<(Url, Vec<CacheTag>)>,
) -> u64 {
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{
            Hash,
            Hasher,
        },
        time::{
            SystemTime,
            UNIX_EPOCH,
        },
    };

    fn mix64(mut value: u64) -> u64 {
        value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
        ^ ((std::process::id() as u64) << 32);

    let mut randomized = parsed
        .drain(..)
        .enumerate()
        .map(|(index, (url, tags))| {
            let mut hasher = DefaultHasher::new();

            seed.hash(&mut hasher);
            index.hash(&mut hasher);
            url.as_str().hash(&mut hasher);

            let order_key = mix64(hasher.finish());

            (order_key, url, tags)
        })
        .collect::<Vec<_>>();

    randomized.sort_by_key(|(order_key, _, _)| *order_key);

    parsed.extend(
        randomized
            .into_iter()
            .map(|(_, url, tags)| (url, tags)),
    );

    seed
}
