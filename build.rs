// Embeds the app icon (assets/icon.ico) into the exe so Explorer, Start and the installer show it.
fn main() {
    embed_resource::compile("assets/icon.rc", embed_resource::NONE);
}
