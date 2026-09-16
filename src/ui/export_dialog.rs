// TODO: Task 11 实现窗口；以下为临时实现
pub enum Kind { Curl, Python }

pub fn render(app: &mut crate::ui::app::FirebeeApp, kind: Kind) -> String {
    let (req, _) = crate::core::vars::substitute_request(&app.current, &app.env_vars());
    let url = crate::core::http::build_url(&req).unwrap_or_else(|_| req.url.clone());
    match kind {
        Kind::Curl => crate::core::export::to_curl(&req, &url),
        Kind::Python => crate::core::export::to_python(&req, &url),
    }
}
