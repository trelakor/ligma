# Figwright plugin code

`code.js` is the built plugin code (`dist/code.js`) of [Figwright](https://github.com/awdr74100/figwright), from the `figwright-plugin-v0.7.0.zip` asset of its v0.7.0 release, unchanged. `VERSION` holds that version. MIT licensed, see `LICENSE.md`.

Ligma's `src/mcp_bridge.rs` runs it in Figma tabs without its window, and talks to Figwright's MCP server (`@figwright/mcp` on npm) in place of the plugin's window. Settings shows the MCP server at the same version, since the server's tools have to match this code.

To update, download the new release's zip, copy its `dist/code.js` over this one, and put the new version in `VERSION`. Then check `packages/shared/src/protocol.ts` (`PROTOCOL_VERSION`), `packages/plugin/protocol/bridge.ts` and `packages/plugin/protocol/host-yield.ts` for changes to the messages `mcp_bridge.rs` sends.

It includes [Zod](https://github.com/colinhacks/zod), bundled by Figwright:

```
MIT License

Copyright (c) 2025 Colin McDonnell

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
