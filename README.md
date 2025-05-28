# Install
`cargo install --git https://github.com/scottopell/nightlies --bin nightlies`

```
Lists the most recent agent-dev nightly images and a GH link for each

Usage: nightlies [OPTIONS]

Options:
  -a, --all-tags
          Show tag details including pushed date and digest
  -p, --print-digest
          Print the image digest for each tag
      --agent-sha <AGENT_SHA>
          Given a sha that exists in the 'main' branch of the datadog-agent repo, print the first nightly that contains that sha
          EXPERIMENTAL - there are known bugs, use at your own risk
      --no-fetch
          Skip git fetch operations (faster but might miss recent updates)
      --force-fetch
          Force git fetch operations even if recently performed
      --num-registry-pages <NUM_REGISTRY_PAGES>
          Number of pages to fetch from the docker registry API
      --latest-only
          Show only most recently published nightly in full URI format
      --prev-latest-only
          Show only the 2nd most recently published nightly in full URI format
      --include-weekends
          Include weekend builds (Saturday/Sunday in UTC)
      --diff-nightlies
          Show a concise diff between the two most recent nightlies
      --diff-interactive
          Interactively select nightlies to diff
  -h, --help
          Print help
  -V, --version
          Print version
```

## Example Usage
```
nightlies
2023-12-27T19:48:28.785149Z  INFO nightlies: Hello, world!
Name: nightly-main-py3, Last Pushed: 2023-12-27T04:16:25.592017+00:00
Name: nightly-main-c9456471-py3, Last Pushed: 2023-12-27T04:16:25.291765+00:00, GitHub URL: https://github.com/DataDog/datadog-agent/tree/c9456471
Name: nightly-main-e4acb3f1-py3, Last Pushed: 2023-12-26T04:15:28.785892+00:00, GitHub URL: https://github.com/DataDog/datadog-agent/tree/e4acb3f1
Name: nightly-main-bd53dc3f-py3, Last Pushed: 2023-12-25T04:16:31.373255+00:00, GitHub URL: https://github.com/DataDog/datadog-agent/tree/bd53dc3f
Name: nightly-main-26065b2a-py3, Last Pushed: 2023-12-24T04:16:18.709055+00:00, GitHub URL: https://github.com/DataDog/datadog-agent/tree/26065b2a
```

### Show detailed tag information
```
nightlies --all-tags
```

### Compare recent nightlies
```
nightlies --diff-nightlies
```

### Get specific nightly by SHA
```
nightlies --agent-sha abc123def
```

## Releasing
To create a new release:

1. Create and push a tag that starts with 'v':
```
git tag v1.0.0
git push origin v1.0.0
```

2. The GitHub Actions workflow will automatically build and release the binary.