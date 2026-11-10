# Figma MCP Bridge plugin code

`code.js` is the built plugin code (`plugin/dist/code.js`) of [Figma MCP Bridge](https://github.com/gethopp/figma-mcp-bridge), from the `figma-mcp-bridge-v0.0.26.zip` asset of its v0.0.26 release, unchanged. `VERSION` holds that version. MIT licensed, see `LICENSE.md`.

Ligma's `src/mcp_bridge.rs` runs it in Figma tabs without its window, and talks to the bridge's MCP server (`@gethopp/figma-mcp-bridge` on npm) in its place. Settings shows the MCP server at the same version, since the server's tools have to match this code.

To update, download the new release's zip, copy its `plugin/dist/code.js` over this one, and put the new version in `VERSION`.

It includes the figma-side renderer of [html-figma](https://github.com/sergcen/html-to-figma) 0.3.1, vendored by the bridge:

```
MIT License

Copyright (c) Sergei Savelev

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
