# Plugins

Plugins add importers, for more banks' statements, and exporters, to other formats. PennyChest reads CSV files itself; plugins handle the rest, such as HSBC's PDF statements or exporting to Beancount.

A plugin is a separate program that PennyChest runs and talks to over its standard input and output. That means:

- it can be written in any language
- a plugin that crashes can't take PennyChest down with it
- a plugin brings its own dependencies, which can't clash with PennyChest's

## Installing plugins

**Settings > Plugins** lists the plugins in the official repository,
[pennychest-plugins](https://github.com/pennychest/pennychest-plugins), and in any
others you've added. **Install** downloads one, checks it against the checksum its repository
lists, and makes it available straight away: its importers appear on the import page, and its
exporters on the export page. **Update** and **Remove** do what they say.

Most plugins are WebAssembly modules, which PennyChest runs in a sandbox: they see the statement
or ledger they're sent, and nothing else, not your files and not the network. Some run as
ordinary programs on your server instead, and those can do anything PennyChest can. Add
another repository only if you trust whoever runs it. To offer only the official repository,
set `PENNYCHEST_ALLOW_PLUGIN_REPOSITORIES=false`.

## Making your own

[The plugin protocol](../plugins/protocol.md) describes everything a plugin does, so you can write one from that page alone. In Rust, the `pennychest-plugin` crate's SDK does the protocol for you: implement an importer or an exporter, and call `serve()`. The protocol page has a complete example of each, an importer for QIF statements in Rust and one for tab-separated files in Python, and shows how to test a plugin by sending it requests by hand.

Each plugin comes with a `pennychest-plugin.toml` manifest saying what it is and how to run it. [Publishing a plugin](../plugins/protocol.md#publishing-a-plugin) covers making a repository others can install it from.
