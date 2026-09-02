# Bundled root certificates

`russian_trusted_root_ca.pem` and `russian_trusted_sub_ca.pem` are the root and
intermediate certificates published by the Russian Ministry of Digital
Development at <https://gu-st.ru/content/lending/>. A number of Russian sites
(gosuslugi.ru, mos.ru and others) present certificates issued under this chain,
which no browser or operating system trust store carries. Without these files
those sites fail to connect at all.

They are **not** added to the default trust set. A connection is verified
against the standard public roots first; only if that fails does the client
retry against a set that also includes these. A government-run authority can
issue a certificate for any domain, so trusting one unconditionally would let it
intercept every site. The fallback ordering means these roots are consulted only
where the standard chain has already been rejected.

Run `mar certs --check` to see the expiry dates of what is bundled.
