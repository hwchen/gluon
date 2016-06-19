use std::fmt;
use std::any::Any;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::result::Result as StdResult;

use base::symbol::Symbol;
use types::*;
use interner::InternedStr;
use gc::{Gc, GcPtr, Traverseable, DataDef, WriteOnly};
use array::{Array, Str};
use thread::{Thread, Status};
use {Error, Result};

use self::Value::{Int, Float, String, Function, PartialApplication, Closure};

mopafy!(Userdata);
pub trait Userdata: ::mopa::Any + Traverseable + Send + Sync {}

impl PartialEq for Userdata {
    fn eq(&self, other: &Userdata) -> bool {
        self as *const _ == other as *const _
    }
}

impl<T> Userdata for T where T: Any + Traverseable + Send + Sync {}

impl fmt::Debug for Userdata {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Userdata")
    }
}

#[derive(Debug)]
pub struct ClosureData {
    pub function: GcPtr<BytecodeFunction>,
    pub upvars: Array<Value>,
}

impl PartialEq for ClosureData {
    fn eq(&self, _: &ClosureData) -> bool {
        false
    }
}

impl Traverseable for ClosureData {
    fn traverse(&self, gc: &mut Gc) {
        self.function.traverse(gc);
        self.upvars.traverse(gc);
    }
}

pub struct ClosureDataDef<'b>(pub GcPtr<BytecodeFunction>, pub &'b [Value]);
impl<'b> Traverseable for ClosureDataDef<'b> {
    fn traverse(&self, gc: &mut Gc) {
        self.0.traverse(gc);
        self.1.traverse(gc);
    }
}

unsafe impl<'b> DataDef for ClosureDataDef<'b> {
    type Value = ClosureData;
    fn size(&self) -> usize {
        use std::mem::size_of;
        size_of::<GcPtr<BytecodeFunction>>() + Array::<Value>::size_of(self.1.len())
    }
    fn initialize<'w>(self, mut result: WriteOnly<'w, ClosureData>) -> &'w mut ClosureData {
        unsafe {
            let result = &mut *result.as_mut_ptr();
            result.function = self.0;
            result.upvars.initialize(self.1.iter().cloned());
            result
        }
    }
}

pub struct ClosureInitDef(pub GcPtr<BytecodeFunction>, pub usize);

impl Traverseable for ClosureInitDef {
    fn traverse(&self, gc: &mut Gc) {
        self.0.traverse(gc);
    }
}

unsafe impl DataDef for ClosureInitDef {
    type Value = ClosureData;
    fn size(&self) -> usize {
        use std::mem::size_of;
        size_of::<GcPtr<BytecodeFunction>>() + Array::<Value>::size_of(self.1)
    }
    fn initialize<'w>(self, mut result: WriteOnly<'w, ClosureData>) -> &'w mut ClosureData {
        use std::ptr;
        unsafe {
            let result = &mut *result.as_mut_ptr();
            result.function = self.0;
            result.upvars.set_len(self.1);
            for var in &mut result.upvars {
                ptr::write(var, Int(0));
            }
            result
        }
    }
}


#[derive(Debug)]
pub struct BytecodeFunction {
    pub name: Symbol,
    pub args: VMIndex,
    pub instructions: Vec<Instruction>,
    pub inner_functions: Vec<GcPtr<BytecodeFunction>>,
    pub strings: Vec<InternedStr>,
    pub globals: Vec<Value>,
}

impl Traverseable for BytecodeFunction {
    fn traverse(&self, gc: &mut Gc) {
        self.inner_functions.traverse(gc);
        self.globals.traverse(gc);
    }
}

pub struct DataStruct {
    pub tag: VMTag,
    pub fields: Array<Value>,
}

impl Traverseable for DataStruct {
    fn traverse(&self, gc: &mut Gc) {
        self.fields.traverse(gc);
    }
}

impl PartialEq for DataStruct {
    fn eq(&self, other: &DataStruct) -> bool {
        self.tag == other.tag && self.fields == other.fields
    }
}

/// Definition for data values in the VM
pub struct Def<'b> {
    pub tag: VMTag,
    pub elems: &'b [Value],
}
unsafe impl<'b> DataDef for Def<'b> {
    type Value = DataStruct;
    fn size(&self) -> usize {
        use std::mem::size_of;
        size_of::<usize>() + Array::<Value>::size_of(self.elems.len())
    }
    fn initialize<'w>(self, mut result: WriteOnly<'w, DataStruct>) -> &'w mut DataStruct {
        unsafe {
            let result = &mut *result.as_mut_ptr();
            result.tag = self.tag;
            result.fields.initialize(self.elems.iter().cloned());
            result
        }
    }
}

impl<'b> Traverseable for Def<'b> {
    fn traverse(&self, gc: &mut Gc) {
        self.elems.traverse(gc);
    }
}

#[derive(Copy, Clone, PartialEq)]
pub enum Value {
    Int(VMInt),
    Float(f64),
    String(GcPtr<Str>),
    Data(GcPtr<DataStruct>),
    Function(GcPtr<ExternFunction>),
    Closure(GcPtr<ClosureData>),
    PartialApplication(GcPtr<PartialApplicationData>),
    Userdata(GcPtr<Box<Userdata>>),
    Thread(GcPtr<Thread>),
}

impl Value {
    pub fn generation(self) -> usize {
        match self {
            String(p) => p.generation(),
            Value::Data(p) => p.generation(),
            Function(p) => p.generation(),
            Closure(p) => p.generation(),
            PartialApplication(p) => p.generation(),
            Value::Userdata(p) => p.generation(),
            Value::Thread(p) => p.generation(),
            Int(_) | Float(_) => 0,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub enum Callable {
    Closure(GcPtr<ClosureData>),
    Extern(GcPtr<ExternFunction>),
}

impl Callable {
    pub fn name(&self) -> &Symbol {
        match *self {
            Callable::Closure(ref closure) => &closure.function.name,
            Callable::Extern(ref ext) => &ext.id,
        }
    }

    pub fn args(&self) -> VMIndex {
        match *self {
            Callable::Closure(ref closure) => closure.function.args,
            Callable::Extern(ref ext) => ext.args,
        }
    }
}

impl PartialEq for Callable {
    fn eq(&self, _: &Callable) -> bool {
        false
    }
}

impl Traverseable for Callable {
    fn traverse(&self, gc: &mut Gc) {
        match *self {
            Callable::Closure(ref closure) => closure.traverse(gc),
            Callable::Extern(_) => (),
        }
    }
}

#[derive(Debug)]
pub struct PartialApplicationData {
    pub function: Callable,
    pub arguments: Array<Value>,
}

impl PartialEq for PartialApplicationData {
    fn eq(&self, _: &PartialApplicationData) -> bool {
        false
    }
}

impl Traverseable for PartialApplicationData {
    fn traverse(&self, gc: &mut Gc) {
        self.function.traverse(gc);
        self.arguments.traverse(gc);
    }
}

pub struct PartialApplicationDataDef<'b>(pub Callable, pub &'b [Value]);
impl<'b> Traverseable for PartialApplicationDataDef<'b> {
    fn traverse(&self, gc: &mut Gc) {
        self.0.traverse(gc);
        self.1.traverse(gc);
    }
}
unsafe impl<'b> DataDef for PartialApplicationDataDef<'b> {
    type Value = PartialApplicationData;
    fn size(&self) -> usize {
        use std::mem::size_of;
        size_of::<Callable>() + Array::<Value>::size_of(self.1.len())
    }
    fn initialize<'w>(self,
                      mut result: WriteOnly<'w, PartialApplicationData>)
                      -> &'w mut PartialApplicationData {
        unsafe {
            let result = &mut *result.as_mut_ptr();
            result.function = self.0;
            result.arguments.initialize(self.1.iter().cloned());
            result
        }
    }
}

impl Traverseable for Value {
    fn traverse(&self, gc: &mut Gc) {
        match *self {
            String(ref data) => data.traverse(gc),
            Value::Data(ref data) => data.traverse(gc),
            Function(ref data) => data.traverse(gc),
            Closure(ref data) => data.traverse(gc),
            Value::Userdata(ref data) => data.traverse(gc),
            PartialApplication(ref data) => data.traverse(gc),
            Value::Thread(ref thread) => thread.traverse(gc),
            Int(_) | Float(_) => (),
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        struct Level<'b>(i32, &'b Value);
        struct LevelSlice<'b>(i32, &'b [Value]);
        impl<'b> fmt::Debug for LevelSlice<'b> {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                let level = self.0;
                if level <= 0 || self.1.is_empty() {
                    return Ok(());
                }
                try!(write!(f, "{:?}", Level(level - 1, &self.1[0])));
                for v in &self.1[1..] {
                    try!(write!(f, ", {:?}", Level(level - 1, v)));
                }
                Ok(())
            }
        }
        impl<'b> fmt::Debug for Level<'b> {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                let level = self.0;
                if level <= 0 {
                    return Ok(());
                }
                match *self.1 {
                    Int(i) => write!(f, "{:?}", i),
                    Float(x) => write!(f, "{:?}f", x),
                    String(x) => write!(f, "{:?}", &*x),
                    Value::Data(ref data) => {
                        write!(f,
                               "{{{:?}: {:?}}}",
                               data.tag,
                               LevelSlice(level - 1, &data.fields))
                    }
                    Function(ref func) => write!(f, "<EXTERN {:?}>", &**func),
                    Closure(ref closure) => {
                        let p: *const _ = &*closure.function;
                        write!(f, "<{:?} {:?}>", closure.function.name, p)
                    }
                    PartialApplication(ref app) => {
                        let name = match app.function {
                            Callable::Closure(_) => "<CLOSURE>",
                            Callable::Extern(_) => "<EXTERN>",
                        };
                        write!(f,
                               "<App {:?}{:?}>",
                               name,
                               LevelSlice(level - 1, &app.arguments))
                    }
                    Value::Userdata(ref data) => write!(f, "<Userdata {:p}>", &**data),
                    Value::Thread(_) => write!(f, "<thread>"),
                }
            }
        }
        write!(f, "{:?}", Level(3, self))
    }
}

pub struct ExternFunction {
    pub id: Symbol,
    pub args: VMIndex,
    pub function: Box<Fn(&Thread) -> Status + Send + Sync>,
}

impl PartialEq for ExternFunction {
    fn eq(&self, _: &ExternFunction) -> bool {
        false
    }
}

impl fmt::Debug for ExternFunction {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // read the v-table pointer of the Fn(..) type and print that
        let p: *const () = unsafe { ::std::mem::transmute_copy(&&*self.function) };
        write!(f, "{:?}", p)
    }
}

impl Traverseable for ExternFunction {
    fn traverse(&self, _: &mut Gc) {}
}

fn deep_clone_ptr<T, A>(value: GcPtr<T>,
                        visited: &mut HashMap<*const (), Value>,
                        alloc: A)
                        -> StdResult<Value, GcPtr<T>>
    where A: FnOnce(&T) -> (Value, GcPtr<T>)
{
    let key = &*value as *const T as *const ();
    let new_ptr = match visited.entry(key) {
        Entry::Occupied(entry) => return Ok(*entry.get()),
        Entry::Vacant(entry) => {
            // FIXME Should allocate the real `Value` and possibly fill it later
            let (value, new_ptr) = alloc(&value);
            entry.insert(value);
            new_ptr
        }
    };
    Err(new_ptr)
}

fn deep_clone_str(data: GcPtr<Str>,
                  visited: &mut HashMap<*const (), Value>,
                  gc: &mut Gc)
                  -> Result<Value> {
    Ok(deep_clone_ptr(data, visited, |data| {
           let ptr = gc.alloc(&data[..]);
           (String(ptr), ptr)
       })
           .unwrap_or_else(String))
}
fn deep_clone_data(data: GcPtr<DataStruct>,
                   visited: &mut HashMap<*const (), Value>,
                   gc: &mut Gc)
                   -> Result<GcPtr<DataStruct>> {
    let result = deep_clone_ptr(data, visited, |data| {
        let ptr = gc.alloc(Def {
            tag: data.tag,
            elems: &data.fields,
        });
        (Value::Data(ptr), ptr)
    });
    match result {
        Ok(Value::Data(ptr)) => Ok(ptr),
        Ok(_) => unreachable!(),
        Err(mut new_data) => {
            {
                let new_fields = unsafe { &mut new_data.as_mut().fields };
                for (new, old) in new_fields.iter_mut().zip(&data.fields) {
                    *new = try!(deep_clone(old, visited, gc));
                }
            }
            Ok(new_data)
        }
    }
}

fn deep_clone_closure(data: GcPtr<ClosureData>,
                      visited: &mut HashMap<*const (), Value>,
                      gc: &mut Gc)
                      -> Result<GcPtr<ClosureData>> {
    let result = deep_clone_ptr(data, visited, |data| {
        let ptr = gc.alloc(ClosureDataDef(data.function, &data.upvars));
        (Closure(ptr), ptr)
    });
    match result {
        Ok(Value::Closure(ptr)) => Ok(ptr),
        Ok(_) => unreachable!(),
        Err(mut new_data) => {
            {
                let new_upvars = unsafe { &mut new_data.as_mut().upvars };
                for (new, old) in new_upvars.iter_mut().zip(&data.upvars) {
                    *new = try!(deep_clone(old, visited, gc));
                }
            }
            Ok(new_data)
        }
    }
}
fn deep_clone_app(data: GcPtr<PartialApplicationData>,
                  visited: &mut HashMap<*const (), Value>,
                  gc: &mut Gc)
                  -> Result<GcPtr<PartialApplicationData>> {
    let result = deep_clone_ptr(data, visited, |data| {
        let ptr = gc.alloc(PartialApplicationDataDef(data.function, &data.arguments));
        (PartialApplication(ptr), ptr)
    });
    match result {
        Ok(Value::PartialApplication(ptr)) => Ok(ptr),
        Ok(_) => unreachable!(),
        Err(mut new_data) => {
            {
                let new_arguments = unsafe { &mut new_data.as_mut().arguments };
                for (new, old) in new_arguments.iter_mut()
                                               .zip(&data.arguments) {
                    *new = try!(deep_clone(old, visited, gc));
                }
            }
            Ok(new_data)
        }
    }
}
pub fn deep_clone(value: &Value,
                  visited: &mut HashMap<*const (), Value>,
                  gc: &mut Gc)
                  -> Result<Value> {
    // Only need to clone values which belong to a younger generation than the gc that the new
    // value will live in
    if value.generation() <= gc.generation() {
        return Ok(*value);
    }
    match *value {
        String(data) => deep_clone_str(data, visited, gc),
        Value::Data(data) => deep_clone_data(data, visited, gc).map(Value::Data),
        Closure(data) => deep_clone_closure(data, visited, gc).map(Value::Closure),
        PartialApplication(data) => {
            deep_clone_app(data, visited, gc).map(Value::PartialApplication)
        }
        Function(_) |
        Value::Userdata(_) |
        Value::Thread(_) => {
            return Err(Error::Message("Threads, Userdata and Extern functions cannot be deep \
                                       cloned yet"
                                          .into()))
        }
        Int(i) => Ok(Int(i)),
        Float(f) => Ok(Float(f)),
    }
}