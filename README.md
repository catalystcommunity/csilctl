# csilctl

A `curl`/`grpcurl`-style CLI for sending arbitrary [CSIL](https://github.com/catalystcommunity/csilgen) messages.

## How it works

`csilctl` parses a `.csil` source file directly (see the [CSIL spec](https://github.com/catalystcommunity/csilgen/blob/main/docs/csil-spec.md)) to discover services, their messages, and each message's request/response/error fields. `list` simply shows you a filtered version of whats in this file. `send` works directly from the CSIL file to send and show responses for RPC messages via the `csilgen_transport` crate.

1. `--client` is a global flag, set before the subcommand. It's a path to the CSIL file.

2. **List messages** — by default, just service and method names plus the file's other types:

   ```sh
   csilctl --client ./corndogs.csil list
   ```
   
   Or name a method or type to see the details for just that item:
   ```sh
   csilctl --client ./corndogs.csil list SubmitTask
   csilctl --client ./corndogs.csil list Task
   ```

2. **Build a message** — pass a JSON-like payload; any required field you leave out is prompted for interactively:

   ```sh
   csilctl --client ./corndogs.csil send --message CreateWidget --data '{"name": "widget-1", "count": 3}'
   ```

3. **Send it** — the payload is marshaled into the client's generated message type and sent to a `host:port`:

   ```sh
   csilctl --client ./corndogs.csil send --message CreateWidget --data '{"name": "widget-1"}' --host example.com:9000
   ```

## Usage (target interface)

| Flag              | Scope  | Description                                                                 |
|-------------------|--------|-----------------------------------------------------------------------------|
| `--client`        | global | For `list`: path to a `.csil` source file. For `send`: path to a folder containing a pre-generated csilgen Go client |
| `--disable-color` | global | Disable colorized output (see priority order below)                         |
| `--message`       | `send` | Name of the message/operation to send                                       |
| `--data`          | `send` | JSON-like payload for the message; missing required fields are prompted for |
| `--host`          | `send` | Destination host/domain + port in `host:port` format                        |

| Environment variable | Description                                                    |
|-----------------------|-----------------------------------------------------------------|
| `NO_COLOR`            | Any value disables colorized output, overriding `FORCE_COLOR` and `--disable-color` |
| `FORCE_COLOR`         | Any value enables colorized output, overriding `--disable-color` (but not `NO_COLOR`) |

`list` output is colorized by default. `NO_COLOR` beats `FORCE_COLOR` beats `--disable-color` — e.g. `FORCE_COLOR=1 csilctl --client ./corndogs.csil --disable-color list` still prints in color, since `FORCE_COLOR` wins over the flag.

This is the intended CLI surface for the initial implementation and may evolve.

## Dev flow

CI/CD runs on [Reactorcide](https://github.com/catalystcommunity/reactorcide/), with releases versioned and tagged via [semver-tags](https://github.com/catalystcommunity/semver-tags). Pull requests will be checked with [conventional commits](https://www.conventionalcommits.org/en/v1.0.0/)

## Project structure

- `cli/` the main CLI code. I imagine everything is in there for now.

## Status

Currently in development, and dogfooding as I use this tool to debug some stuff. CLI is built on [`clap`](https://github.com/clap-rs/clap). Just chat first if you wanna make changes.
