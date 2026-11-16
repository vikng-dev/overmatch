fn main() -> bevy::app::AppExit {
    if std::env::args().any(|a| a == "--offline") {
        overmatch::run_offline()
    } else {
        overmatch::run_client()
    }
}
