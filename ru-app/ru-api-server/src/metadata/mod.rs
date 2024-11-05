use actix_web::{get, post, put, delete, web, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone)]
pub struct Metadata {
    pub id: String,
    pub name: String,
    pub value: String,
}

pub struct AppState {
    pub metadata_store: Mutex<HashMap<String, HashMap<String, Metadata>>>,
}

#[get("/{type}/metadata")]
pub async fn get_metadata(state: web::Data<AppState>, type_: web::Path<String>) -> impl Responder {
    let store = state.metadata_store.lock().unwrap();
    let metadata_list = store.get(&type_.to_string()).map_or(vec![], |m| m.values().cloned().collect::<Vec<_>>());
    HttpResponse::Ok().json(metadata_list)
}

#[post("/{type}/metadata")]
pub async fn create_metadata(state: web::Data<AppState>, type_: web::Path<String>, item: web::Json<Metadata>) -> impl Responder {
    let mut store = state.metadata_store.lock().unwrap();
    store.entry(type_.to_string()).or_default().insert(item.id.clone(), item.into_inner());
    HttpResponse::Created().finish()
}

#[get("/{type}/metadata/{id}")]
pub async fn get_metadata_by_id(state: web::Data<AppState>, path: web::Path<(String, String)>) -> impl Responder {
    let (type_, id) = path.into_inner();
    let store = state.metadata_store.lock().unwrap();

    match store.get(&type_) {
        Some(metadata_map) => {
            match metadata_map.get(&id) {
                Some(metadata) => HttpResponse::Ok().json(metadata),
                None => HttpResponse::NotFound().finish(),
            }
        },
        None => HttpResponse::NotFound().finish(),
    }
}

#[put("/{type}/metadata/{id}")]
pub async fn update_metadata(state: web::Data<AppState>, path: web::Path<(String, String)>, item: web::Json<Metadata>) -> impl Responder {
    let (type_, id) = path.into_inner();
    let mut store = state.metadata_store.lock().unwrap();

    if let Some(metadata_map) = store.get_mut(&type_) {
        if metadata_map.contains_key(&id) {
            metadata_map.insert(id, item.into_inner());
            return HttpResponse::Ok().finish();
        }
    }
    HttpResponse::NotFound().finish()
}

#[delete("/{type}/metadata/{id}")]
pub async fn delete_metadata(state: web::Data<AppState>, path: web::Path<(String, String)>) -> impl Responder {
    let (type_, id) = path.into_inner();
    let mut store = state.metadata_store.lock().unwrap();

    if let Some(metadata_map) = store.get_mut(&type_) {
        if metadata_map.remove(&id).is_some() {
            return HttpResponse::NoContent().finish();
        }
    }
    HttpResponse::NotFound().finish()
}
