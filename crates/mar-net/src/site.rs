//! Which hosts count as the same site, and which are worth nobody's time.
//!
//! A page and its code are routinely on different hosts: the document on
//! `app.example.com`, the bundle on `static.example.com`, the API on
//! `api.example.com`. Comparing origins puts all three in different buckets, so
//! a rule written as "same origin only" blocks an application from loading
//! itself. Comparing registrable domains puts them back together, which is what
//! `SameSite` cookies, CSP `'self'`-adjacent tooling and every CDN layout
//! assume.
//!
//! The other half is the reason the origin rule existed: third-party analytics.
//! That is a blocklist, not a topology question, so it is one here too.

/// Suffixes under which registrations happen at the third level.
///
/// The real answer is the Public Suffix List, which is a megabyte of data that
/// changes weekly. These are the ones that actually turn up in a corpus of
/// pages; everything else registers at the second level, which is the default.
const MULTI_LABEL_SUFFIXES: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "net.uk", "sch.uk", "com.au", "net.au",
    "org.au", "edu.au", "gov.au", "co.nz", "net.nz", "org.nz", "govt.nz", "co.za", "org.za",
    "co.jp", "or.jp", "ne.jp", "ac.jp", "go.jp", "com.br", "net.br", "org.br", "gov.br",
    "com.cn", "net.cn", "org.cn", "gov.cn", "edu.cn", "com.hk", "com.sg", "com.tr", "com.mx",
    "com.ar", "com.tw", "co.kr", "or.kr", "co.in", "net.in", "org.in", "gov.in", "com.ua",
    "com.pl", "com.es", "com.it", "co.il", "com.my", "com.ph", "com.vn", "com.co", "com.pe",
    "com.eg", "com.sa", "com.ng", "com.pk", "com.bd", "com.ec", "com.uy", "com.ve", "com.do",
    // The Russian, Ukrainian and Kazakh third-level zones, common in this corpus.
    "org.ru", "net.ru", "pp.ru", "com.ru", "msk.ru", "spb.ru", "edu.ru", "gov.ru", "int.ru",
    "ac.ru", "org.ua", "net.ua", "kiev.ua", "org.kz", "net.kz",
    // Hosting suffixes where neighbours are unrelated parties.
    "github.io", "gitlab.io", "pages.dev", "workers.dev", "vercel.app", "netlify.app",
    "herokuapp.com", "azurewebsites.net", "cloudfront.net", "s3.amazonaws.com",
    "web.app", "firebaseapp.com", "appspot.com", "blogspot.com", "wordpress.com",
    "myshopify.com", "surge.sh", "fly.dev", "onrender.com", "koyeb.app",
];

/// The registrable part of a host: `api.example.co.uk` -> `example.co.uk`.
///
/// An IP literal and a single-label host are their own registrable domain,
/// because there is nothing to strip.
pub fn registrable_domain(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() {
        return host;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 3 {
        return host;
    }
    let last_two = labels[labels.len() - 2..].join(".");
    let take = if MULTI_LABEL_SUFFIXES.contains(&last_two.as_str()) {
        3
    } else {
        2
    };
    if labels.len() <= take {
        return host;
    }
    labels[labels.len() - take..].join(".")
}

/// Do these two hosts belong to the same party?
pub fn same_site(a: &str, b: &str) -> bool {
    !a.is_empty() && registrable_domain(a) == registrable_domain(b)
}

/// Third-party hosts whose only job is measurement, advertising or chat.
///
/// Fetching these costs a request and a parse and changes nothing a reader
/// will ever see. Matching is on the registrable domain, so every regional
/// shard and CDN alias of one of these is covered by the single entry.
const TRACKER_DOMAINS: &[&str] = &[
    // Analytics and tag management.
    "google-analytics.com", "googletagmanager.com", "googletagservices.com", "googlesyndication.com",
    "googleadservices.com", "doubleclick.net", "adservice.google.com", "analytics.google.com",
    "segment.com", "segment.io", "amplitude.com", "mixpanel.com", "heapanalytics.com",
    "fullstory.com", "hotjar.com", "hotjar.io", "clarity.ms", "mouseflow.com", "luckyorange.com",
    "quantserve.com", "scorecardresearch.com", "chartbeat.com", "chartbeat.net", "parsely.com",
    "newrelic.com", "nr-data.net", "sentry.io", "bugsnag.com", "rollbar.com", "datadoghq.com",
    "logrocket.com", "logrocket.io", "smartlook.com", "matomo.cloud", "statcounter.com",
    "yandex.ru/metrika", "mc.yandex.ru", "top-fwz1.mail.ru", "top.mail.ru", "tns-counter.ru",
    "mediascope.net", "adriver.ru", "rambler.ru/counter", "vk.com/rtrg", "criteo.com", "criteo.net",
    // Advertising exchanges.
    "adnxs.com", "rubiconproject.com", "pubmatic.com", "openx.net", "casalemedia.com",
    "taboola.com", "outbrain.com", "sharethrough.com", "smartadserver.com", "adform.net",
    "yieldmo.com", "indexww.com", "3lift.com", "bidswitch.net", "adsrvr.org", "everesttech.net",
    "moatads.com", "adsafeprotected.com", "serving-sys.com", "flashtalking.com", "teads.tv",
    "buysellads.com", "carbonads.net", "media.net", "amazon-adsystem.com", "buzzoola.com",
    "sape.ru", "relap.io", "digitaltarget.ru", "betweendigital.com", "rtb-media.ru",
    // Social pixels and share widgets.
    "facebook.net", "connect.facebook.net", "fbcdn.net", "twitter.com/i/adsct", "ads-twitter.com",
    "t.co", "linkedin.com/px", "licdn.com/li.lms-analytics", "snapchat.com", "tiktok.com/i18n",
    "analytics.tiktok.com", "pinterest.com/ct", "reddit.com/pixel", "bat.bing.com", "clarity.microsoft.com",
    // Consent, chat and support widgets: heavy, and never part of the article.
    "cookiebot.com", "cookielaw.org", "onetrust.com", "trustarc.com", "usercentrics.eu",
    "intercom.io", "intercomcdn.com", "zdassets.com", "zopim.com", "livechatinc.com",
    "crisp.chat", "tawk.to", "drift.com", "hubspot.com", "hs-scripts.com", "hs-analytics.net",
    "jivosite.com", "jivo.ru", "carrotquest.io", "usedesk.ru", "verbox.ru",
    "marketo.net", "pardot.com", "mktoresp.com", "omtrdc.net", "demdex.net", "2o7.net",
];

/// Is this host one of the known third-party trackers?
pub fn is_tracker(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let domain = registrable_domain(&host);
    TRACKER_DOMAINS.iter().any(|entry| {
        // Entries carrying a path are a host plus a hint about which part of
        // it matters; only the host is checked here.
        let entry = entry.split('/').next().unwrap_or(entry);
        entry == domain || entry == host || host.ends_with(&format!(".{entry}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registrable_domain_is_the_part_a_party_registers() {
        assert_eq!(registrable_domain("app.ladcraft.ru"), "ladcraft.ru");
        assert_eq!(registrable_domain("api.ladcraft.ru"), "ladcraft.ru");
        assert_eq!(registrable_domain("example.com"), "example.com");
        assert_eq!(registrable_domain("a.b.c.example.com"), "example.com");
        assert_eq!(registrable_domain("www.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(registrable_domain("shop.example.com.au"), "example.com.au");
        assert_eq!(registrable_domain("localhost"), "localhost");
        assert_eq!(registrable_domain("93.184.216.34"), "93.184.216.34");
    }

    #[test]
    fn a_project_on_a_shared_host_is_not_the_neighbours_site() {
        assert!(!same_site("alice.github.io", "bob.github.io"));
        assert!(same_site("alice.github.io", "alice.github.io"));
    }

    #[test]
    fn same_site_spans_subdomains_and_stops_at_the_domain() {
        assert!(same_site("app.ladcraft.ru", "api.ladcraft.ru"));
        assert!(same_site("static.example.com", "example.com"));
        assert!(!same_site("example.com", "notexample.com"));
        assert!(!same_site("example.com", "example.org"));
    }

    #[test]
    fn a_sites_own_host_is_never_somebody_elses_tracker() {
        // A site self-hosting its measurement under its own domain, or one
        // whose name simply collides with a blocklist entry, is still the
        // page's own code and has to load.
        assert!(same_site("analytics.google.com", "www.google.com"));
        assert!(is_tracker("analytics.google.com"));
        assert!(!same_site("analytics.google.com", "example.com"));
    }

    #[test]
    fn trackers_are_matched_by_domain_not_by_exact_host() {
        assert!(is_tracker("www.google-analytics.com"));
        assert!(is_tracker("region1.google-analytics.com"));
        assert!(is_tracker("mc.yandex.ru"));
        assert!(is_tracker("connect.facebook.net"));
        assert!(!is_tracker("example.com"));
        assert!(!is_tracker("cdn.jsdelivr.net"));
        assert!(!is_tracker("unpkg.com"));
    }
}
