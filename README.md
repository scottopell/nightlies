```
Agent Nightly Images 1.0
Scott Opell <me@scottopell.com>
Scrapes and lists the recent agent nightly images and a GH link for each.

USAGE:
    nightlies [SUBCOMMAND]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    help      Prints this message or the help of the given subcommand(s)
    query     Queries the database for nightlies
    update    Updates the database with new nightlies
```

## Releasing
A new binary can be built by the `release` github workflow by pushing a tag that starts with `v`.

eg:
```
git tag v1.0.0
tig push origin v1.0.0
```