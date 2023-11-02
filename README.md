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

## Example Usage
```
./target/release/nightlies
Name: nightly-main-py3, Last Pushed: 2023-11-02T04:21:03.804663Z
Name: nightly-main-1bf80594-py3, Last Pushed: 2023-11-02T04:21:03.578999Z, GitHub URL: https://github.com/DataDog/datadog-agent/tree/1bf80594
Name: nightly-main-a5ef9a77-py3, Last Pushed: 2023-11-01T04:10:01.535483Z, GitHub URL: https://github.com/DataDog/datadog-agent/tree/a5ef9a77
Name: nightly-main-9610a269-py3, Last Pushed: 2023-10-31T04:10:23.476751Z, GitHub URL: https://github.com/DataDog/datadog-agent/tree/9610a269
Name: nightly-main-70ca5d9b-py3, Last Pushed: 2023-10-30T04:13:31.875125Z, GitHub URL: https://github.com/DataDog/datadog-agent/tree/70ca5d9b
Name: nightly-main-fb59d07f-py3, Last Pushed: 2023-10-29T04:09:48.9653Z, GitHub URL: https://github.com/DataDog/datadog-agent/tree/fb59d07f
Name: nightly-main-17ac27f0-py3, Last Pushed: 2023-10-27T04:12:54.595658Z, GitHub URL: https://github.com/DataDog/datadog-agent/tree/17ac27f0
Name: nightly-main-ac7747e4-py3, Last Pushed: 2023-10-26T04:10:50.862523Z, GitHub URL: https://github.com/DataDog/datadog-agent/tree/ac7747e4
Name: nightly-main-c4590e26-py3, Last Pushed: 2023-10-25T04:07:46.154271Z, GitHub URL: https://github.com/DataDog/datadog-agent/tree/c4590e26
Name: nightly-main-ffde3207-py3, Last Pushed: 2023-10-24T05:10:44.26368Z, GitHub URL: https://github.com/DataDog/datadog-agent/tree/ffde3207
```

## Releasing
A new binary can be built by the `release` github workflow by pushing a tag that starts with `v`.

eg:
```
git tag v1.0.0
tig push origin v1.0.0
```