```
Lists the most recent agent-dev nightly images and a GH link for each

Usage: nightlies [OPTIONS]

Options:
  -a, --all-tags                 Include all tags, not just those ending in -py3
  -p, --print-digest             Print the image digest for each tag
      --target-sha <TARGET_SHA>  If the given target_sha exists as a nightly, print the tag
  -f, --from-date <FROM_DATE>    Start date for query (inclusive), format: YYYY-MM-DDTHH:MM:SS
  -t, --to-date <TO_DATE>        End date for query (inclusive), format: YYYY-MM-DDTHH:MM:SS
  -h, --help                     Print help
  -V, --version                  Print version
```

## Example Usage
```
./target/release/nightlies
2023-12-27T19:48:28.785149Z  INFO nightlies: Hello, world!
Name: nightly-main-py3, Last Pushed: 2023-12-27T04:16:25.592017+00:00
Name: nightly-main-c9456471-py3, Last Pushed: 2023-12-27T04:16:25.291765+00:00, GitHub URL: https://github.com/DataDog/datadog-agent/tree/c9456471
Name: nightly-main-e4acb3f1-py3, Last Pushed: 2023-12-26T04:15:28.785892+00:00, GitHub URL: https://github.com/DataDog/datadog-agent/tree/e4acb3f1
Name: nightly-main-bd53dc3f-py3, Last Pushed: 2023-12-25T04:16:31.373255+00:00, GitHub URL: https://github.com/DataDog/datadog-agent/tree/bd53dc3f
Name: nightly-main-26065b2a-py3, Last Pushed: 2023-12-24T04:16:18.709055+00:00, GitHub URL: https://github.com/DataDog/datadog-agent/tree/26065b2a
Name: nightly-main-0dc69c06-py3, Last Pushed: 2023-12-22T04:15:09.061328+00:00, GitHub URL: https://github.com/DataDog/datadog-agent/tree/0dc69c06
Name: nightly-main-d50e711a-py3, Last Pushed: 2023-12-21T04:15:30.813378+00:00, GitHub URL: https://github.com/DataDog/datadog-agent/tree/d50e711a
```

## Releasing
> TODO this is broken
A new binary can be built by the `release` github workflow by pushing a tag that starts with `v`.

eg:
```
git tag v1.0.0
tig push origin v1.0.0
```