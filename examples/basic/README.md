# Basic example

Starter layout to copy into a product repository:

```bash
cp -R roadmap projects /path/to/your-product-repo/
```

Then edit `roadmap/roadmap.toml` and fill in `projects/`.

Validate from this tooling repo:

```bash
cargo run --bin roadmap -- --roadmap-dir examples/basic/roadmap validate
```
