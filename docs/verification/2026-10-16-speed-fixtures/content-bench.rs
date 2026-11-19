use std::{alloc::{GlobalAlloc, Layout, System}, sync::atomic::{AtomicBool, AtomicUsize, Ordering}, time::Instant, hint::black_box};
#[allow(dead_code)]
#[path="../../../rust/src/content.rs"] mod current;
#[allow(dead_code)]
#[path="content-before.rs"] mod previous;
static ON: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
struct Alloc;
unsafe impl GlobalAlloc for Alloc {
 unsafe fn alloc(&self, layout:Layout)->*mut u8 { let p=unsafe{System.alloc(layout)}; if !p.is_null() && ON.load(Ordering::Relaxed) {COUNT.fetch_add(1,Ordering::Relaxed);LIVE.fetch_add(layout.size(),Ordering::Relaxed);} p }
 unsafe fn alloc_zeroed(&self, layout:Layout)->*mut u8 { let p=unsafe{System.alloc_zeroed(layout)}; if !p.is_null() && ON.load(Ordering::Relaxed) {COUNT.fetch_add(1,Ordering::Relaxed);LIVE.fetch_add(layout.size(),Ordering::Relaxed);} p }
 unsafe fn dealloc(&self,p:*mut u8,layout:Layout) {if ON.load(Ordering::Relaxed){LIVE.fetch_sub(layout.size(),Ordering::Relaxed);} unsafe{System.dealloc(p,layout)}}
 unsafe fn realloc(&self,p:*mut u8,layout:Layout,size:usize)->*mut u8 {let q=unsafe{System.realloc(p,layout,size)};if !q.is_null() && ON.load(Ordering::Relaxed){COUNT.fetch_add(1,Ordering::Relaxed);LIVE.fetch_add(size,Ordering::Relaxed);LIVE.fetch_sub(layout.size(),Ordering::Relaxed);}q}
}
#[global_allocator] static ALLOC:Alloc=Alloc;
fn allocations<T>(source:&str, construct:impl FnOnce(String)->T)->(usize,usize) {let input=source.to_owned();COUNT.store(0,Ordering::Relaxed);LIVE.store(0,Ordering::Relaxed);ON.store(true,Ordering::Relaxed);let data=construct(input);ON.store(false,Ordering::Relaxed);let result=(COUNT.load(Ordering::Relaxed),LIVE.load(Ordering::Relaxed));black_box(&data);drop(data);result}
fn main(){
 let line="record 00000 | tool completed | workspace /fixture/project | result verified\r\n".replace("\r\n","\r\n");
 // Literal fixture keeps all input local; mixed ASCII/Unicode exercises boundaries.
 let source=(line+"日本😀\r\n").repeat(50_000);
 let old=allocations(&source,previous::CodeContent::new);let new=allocations(&source,current::CodeContent::new);
 let data=current::CodeContent::new(source.clone());let baseline=previous::CodeContent::new(source.clone());assert_eq!(data.clipped,baseline.clipped);assert_eq!(data.source(),baseline.source());assert_eq!(data.is_empty(),baseline.is_empty());drop(data);drop(baseline);
 let mut old_times=Vec::new();let mut new_times=Vec::new();
 for round in 0..9 {
  if round%2==0 {
   let input=source.clone();let start=Instant::now();let data=previous::CodeContent::new(input);old_times.push(start.elapsed().as_nanos());black_box(&data);drop(data);
   let input=source.clone();let start=Instant::now();let data=current::CodeContent::new(input);new_times.push(start.elapsed().as_nanos());black_box(&data);drop(data);
  } else {
   let input=source.clone();let start=Instant::now();let data=current::CodeContent::new(input);new_times.push(start.elapsed().as_nanos());black_box(&data);drop(data);
   let input=source.clone();let start=Instant::now();let data=previous::CodeContent::new(input);old_times.push(start.elapsed().as_nanos());black_box(&data);drop(data);
  }
 }
 old_times.sort();new_times.sort();println!("{{\"fixture_bytes\":{},\"lines\":{},\"samples\":9,\"previous_median_ns\":{},\"current_median_ns\":{},\"previous_constructor_allocations\":{},\"current_constructor_allocations\":{},\"previous_retained_index_and_line_bytes\":{},\"current_retained_index_and_line_bytes\":{},\"scope\":\"CodeContent constructor only; source input allocation excluded; not native UI or end-to-end speed\"}}",source.len(),source.lines().count(),old_times[4],new_times[4],old.0,new.0,old.1,new.1);
}
