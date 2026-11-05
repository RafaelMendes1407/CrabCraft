fn main() {
    let assets = [
        include_str!("../static/index.html"),
        include_str!("../static/app.css"),
        include_str!("../static/app.js"),
        include_str!("../static/app.ts"),
        include_str!("../static/view.tsx"),
    ];
    assert!(assets.iter().all(|asset| !asset.is_empty()));
}
