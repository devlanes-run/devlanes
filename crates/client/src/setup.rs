//! Which missing `devlanes setup` steps a lane's addresses need, shared by
//! the CLI's notice after `devlanes start` and the TUI's setup prompt, so
//! both name the same steps.

/// The scheme and host of `url` (`https`, `api.acme.localhost`), the host
/// without its port, user info or the brackets of an IPv6 address; `None`
/// without a scheme or host.
pub fn url_host(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split_once(']')?.0,
        None => authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host),
    };
    (!host.is_empty()).then_some((scheme, host))
}

/// Whether `url` needs the setup step `step`, named as `daemon.status` names
/// it in `setup.missing`: `ca` only for an `https://` address, `hosts` and
/// `resolver` only for a hostname (anything but an IP address or plain
/// `localhost`), a step this build does not know for any address.
pub fn step_needed(step: &str, url: &str) -> bool {
    let Some((scheme, host)) = url_host(url) else {
        return false;
    };
    match step {
        "ca" => scheme.eq_ignore_ascii_case("https"),
        "hosts" | "resolver" => {
            !host.eq_ignore_ascii_case("localhost") && host.parse::<std::net::IpAddr>().is_err()
        }
        _ => true,
    }
}

/// The steps of `missing` that one of `urls` needs, in `missing`'s order
/// without repeats, and the host of the first URL that needs one of them.
pub fn needed<'a, S: AsRef<str>>(
    missing: &[S],
    urls: impl IntoIterator<Item = &'a str>,
) -> (Vec<String>, Option<&'a str>) {
    let urls: Vec<&str> = urls.into_iter().collect();
    let mut steps: Vec<String> = Vec::new();
    for step in missing.iter().map(AsRef::as_ref) {
        if !steps.iter().any(|known| known == step) && urls.iter().any(|url| step_needed(step, url))
        {
            steps.push(step.to_owned());
        }
    }
    let first = urls
        .iter()
        .find(|url| steps.iter().any(|step| step_needed(step, url)))
        .and_then(|url| url_host(url))
        .map(|(_, host)| host);
    (steps, first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_drop_ports_user_info_and_brackets() {
        assert_eq!(
            url_host("https://user@api.acme.localhost:8443/path?q#f"),
            Some(("https", "api.acme.localhost"))
        );
        assert_eq!(url_host("http://[::1]:3000/"), Some(("http", "::1")));
        assert_eq!(url_host("acme.localhost"), None);
        assert_eq!(url_host("http://"), None);
    }

    /// The CA only for `https://`, the hosts helper and resolver only for a
    /// hostname, whatever the scheme.
    #[test]
    fn each_step_counts_only_for_the_addresses_it_serves() {
        assert!(step_needed("ca", "https://127.0.0.1:20101"));
        assert!(!step_needed("ca", "http://web.acme.localhost"));
        assert!(step_needed("hosts", "http://web.acme.localhost"));
        assert!(step_needed("resolver", "https://web.acme.test"));
        for plain in [
            "http://localhost:3000",
            "http://127.0.0.1:1",
            "http://[::1]:1",
        ] {
            assert!(!step_needed("hosts", plain), "{plain}");
            assert!(!step_needed("resolver", plain), "{plain}");
        }
        assert!(step_needed("future", "http://127.0.0.1:1"));
        assert!(!step_needed("ca", "not a url"));
    }

    #[test]
    fn needed_steps_keep_their_order_and_name_the_first_url_that_needs_one() {
        let missing = ["ca", "hosts", "ca"];
        let urls = [
            "http://127.0.0.1:20101",
            "http://web.acme.localhost:20102",
            "https://api.acme.localhost",
        ];
        assert_eq!(
            needed(&missing, urls),
            (
                vec!["ca".to_owned(), "hosts".to_owned()],
                Some("web.acme.localhost")
            )
        );
        // An http:// hostname with no CA step missing.
        assert_eq!(
            needed(&["ca"], ["http://web.acme.localhost"]),
            (Vec::new(), None)
        );
        assert_eq!(
            needed(&["ca", "hosts"], ["http://127.0.0.1:1"]),
            (Vec::new(), None)
        );
        assert_eq!(
            needed(
                &["ca"],
                ["http://web.acme.localhost", "https://127.0.0.1:1"]
            ),
            (vec!["ca".to_owned()], Some("127.0.0.1"))
        );
    }
}
