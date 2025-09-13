use std::{
  time::Instant,
  io::{self, Write},
  sync::Arc,
  sync::{
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{fs, sync::oneshot};
use axum::Router;
use axum::routing::get;
use axum::extract::State;

fn fib(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        _ => fib(n - 1) + fib(n - 2),
    }
}

async fn fib_rayon(i: u64, val: String) -> Option<String> {
  let (s, r) = oneshot::channel();
  rayon::spawn(move || {
    let result = fib(i);
    let entry = format!("val: {}, fib: {}", val.trim(), result);
    
    let _ = s.send(entry);
  });
  match r.await {
    Ok(val) => {
    //data_ref.insert(i.to_string(), val).await;
    return Some(val);
    },
    Err(_) => {
      return None;
    }
  }
  
}

async fn spawn(n: u64) -> String {

let result = fib(n);
            let mut val = fs::read_to_string("data.txt").await.unwrap();
            
            val = format!("val: {}, fib: {}", val, result.to_string());
            //data_ref.insert(i.to_string(), val).await;
            return val;


  //let data = Arc::new(whirlwind::ShardMap::new());

    //let mut handles = Vec::new();
    
    //let time = Instant::now();
/*
    for i in 0..n {
        //let data_ref = data.clone();
        let handle = tokio::spawn(async move {
      
            let result = fib(i);
            let mut val = fs::read_to_string("data.txt").await.unwrap();
            
            val = format!("val: {}, fib: {}", val, result.to_string());
            //data_ref.insert(i.to_string(), val).await;
            return val;
            
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }
    */
    
    //let time2 = time.elapsed();
    
    //println!("\ntotal items: {}", data.len().await);
    //println!("\n\nspawn: {:?}\n\n", time2);
}

async fn spawn_blocking(n: u64) -> String {

let mut val = fs::read_to_string("data.txt").await.unwrap();
            
          let (key, val) = tokio::task::spawn_blocking(move || { 
            let result = fib(n);
            val = format!("val: {}, fib: {}", val, result);
            (String::from("20"), val)
          })
          .await
          .unwrap();
          
          return val;


  //let data = Arc::new(whirlwind::ShardMap::new());
/*
    let mut handles = Vec::new();
    
    //let time = Instant::now();

    for i in 0..n {
        //let data_ref = data.clone();
        let handle = tokio::spawn(async move {
        
            let mut val = fs::read_to_string("data.txt").await.unwrap();
            
          let (key, val) = tokio::task::spawn_blocking(move || { 
            let result = fib(i);
            val = format!("val: {}, fib: {}", val, result);
            (i.to_string(), val)
          })
          .await
          .unwrap();
          
          return val;

        //data_ref.insert(key, val).await;
            
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }
    */
    //let time2 = time.elapsed();
    
    //println!("\ntotal items: {}", data.len().await);
    //println!("\n\nspawn blocking: {:?}\n\n", time2);
}

async fn rayon(n: u64) -> String {

  let val = fs::read_to_string("data.txt").await.unwrap();

  let hasil = fib_rayon(n, val).await.unwrap();
  return hasil;


    //let data = Arc::new(whirlwind::ShardMap::new());
    /*
    let mut handles = Vec::new();
    //let time = Instant::now();

    for i in 0..n {
        //let data_ref = data.clone();

        let handle = tokio::spawn(async move {

            let val = fs::read_to_string("data.txt").await.unwrap();

            let hasil = fib_rayon(i, val).await.unwrap();
            return hasil;
        });

        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }
*/
    //let duration = time.elapsed();
    //println!("\ntotal items: {}", data.len().await);
    //println!("\n\nrayon: {:?}\n\n", duration);
}

async fn root() -> String {
    spawn(40).await
}

async fn root2() -> String {
    spawn_blocking(40).await
}

async fn root3() -> String {
    rayon(40).await
}

async fn root4(State(counter): State<Arc<AtomicU64>>) -> String {
    let req_num = counter.fetch_add(1, Ordering::Relaxed) + 1;
    format!("hello, request ke-{}", req_num)
}

#[tokio::main]
async fn main() {
    if !std::path::Path::new("data.txt").exists() {
        fs::write("data.txt", "hello").await.unwrap();
    }
    
    let counter = Arc::new(AtomicU64::new(0));
    
    let app = Router::new()

        .route("/1", get(root))
        .route("/2", get(root2))
        .route("/3", get(root3))
        .route("/4", get(root4))
        .with_state(counter); // inject counter ke semua handler

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    println!("backend is listening on port 8080");
    axum::serve(listener, app).await.unwrap();

    
    /*
    
    loop {
        print!("\n1 - spawn\n2 - spawn_blocking\n3 - rayon\n\ninput: ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let input = input.trim(); 

        match input {
            "1" => spawn(45).await,
            "2" => spawn_blocking(45).await,
            "3" => rayon(45).await,
            "exit" => {
                println!("Exiting program...");
                break;
            }
            _ => println!("Input is not valid"),
        }
    }
    */
}
