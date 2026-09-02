const api = globalThis.browser ?? globalThis.chrome;

api.devtools.panels.create("CSIL", "icon-16.png", "panel.html");
