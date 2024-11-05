mod metadata; // 引入元数据管理相关模块
mod user;     // 引入用户管理相关模块

use actix_web::{web, App, HttpServer};
use std::sync::Mutex;
use std::collections::HashMap;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // 初始化应用状态，包含元数据存储
    let metadata_state = web::Data::new(metadata::AppState {
        metadata_store: Mutex::new(HashMap::new()),
    });

    // 初始化应用状态，包含用户存储
    let user_state = web::Data::new(user::AppState {
        user_store: Mutex::new(HashMap::new()),
    });

    // 启动 HTTP 服务器
    HttpServer::new(move || {
        App::new()
            .app_data(metadata_state.clone()) // 添加元数据状态
            .app_data(user_state.clone())     // 添加用户状态
            .service(metadata::get_metadata)
            .service(metadata::create_metadata)
            .service(metadata::get_metadata_by_id)
            .service(metadata::update_metadata)
            .service(metadata::delete_metadata)
            .service(user::get_users)
            .service(user::create_user)
            .service(user::get_user_by_id)
            .service(user::update_user)
            .service(user::delete_user)
    })
        .bind("127.0.0.1:8080")? // 绑定 IP 和端口
        .run()
        .await
}
