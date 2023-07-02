# Rust binary size analyzer

How to use:

`cd client && npm i && npm run dev`

In a separate shell:

`cargo run --release BINARY_PATH > client/groups.json`

Note: The symbol parsing code is extremely cursed. This may lead to very wonky results.