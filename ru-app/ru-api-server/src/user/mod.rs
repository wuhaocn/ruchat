use actix_web::{get, post, put, delete, web, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone)]
pub struct User {
    pub id: String,
    pub name: String,
    pub email: String,
}

pub struct AppState {
    pub user_store: Mutex<HashMap<String, User>>,
}

#[get("/users")]
pub async fn get_users(state: web::Data<AppState>) -> impl Responder {
    let store = state.user_store.lock().expect("Failed to lock mutex");
    let user_list: Vec<User> = store.values().cloned().collect();
    HttpResponse::Ok().json(user_list)
}

#[post("/users")]
pub async fn create_user(state: web::Data<AppState>, user: web::Json<User>) -> impl Responder {
    let mut store = state.user_store.lock().expect("Failed to lock mutex");
    store.insert(user.id.clone(), user.into_inner());
    HttpResponse::Created().finish()
}

#[get("/users/{id}")]
pub async fn get_user_by_id(state: web::Data<AppState>, path: web::Path<String>) -> impl Responder {
    let store = state.user_store.lock().expect("Failed to lock mutex");
    let user_id = path.into_inner();

    match store.get(&user_id) {
        Some(user) => HttpResponse::Ok().json(user),
        None => HttpResponse::NotFound().finish(),
    }
}

#[put("/users/{id}")]
pub async fn update_user(state: web::Data<AppState>, path: web::Path<String>, user: web::Json<User>) -> impl Responder {
    let mut store = state.user_store.lock().expect("Failed to lock mutex");
    let user_id = path.into_inner();

    if store.contains_key(&user_id) {
        store.insert(user_id.clone(), user.into_inner());
        HttpResponse::Ok().finish()
    } else {
        HttpResponse::NotFound().finish()
    }
}

#[delete("/users/{id}")]
pub async fn delete_user(state: web::Data<AppState>, path: web::Path<String>) -> impl Responder {
    let mut store = state.user_store.lock().expect("Failed to lock mutex");
    let user_id = path.into_inner();

    if store.remove(&user_id).is_some() {
        HttpResponse::NoContent().finish()
    } else {
        HttpResponse::NotFound().finish()
    }
}
