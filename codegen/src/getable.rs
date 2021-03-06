use proc_macro2::TokenStream;
use shared::{map_lifetimes, map_type_params, split_for_impl};
use syn::{
    self, Data, DataEnum, DataStruct, DeriveInput, Field, Fields, FieldsNamed, FieldsUnnamed,
    Generics, Ident, Variant,
};

pub fn derive(input: TokenStream) -> TokenStream {
    let DeriveInput {
        ident,
        data,
        generics,
        ..
    } = syn::parse2(input).expect("Input is checked by rustc");

    let tokens = match data {
        Data::Struct(ast) => derive_struct(ast, ident, generics),
        Data::Enum(ast) => derive_enum(ast, ident, generics),
        Data::Union(_) => panic!("Unions are not supported"),
    };

    tokens.into()
}

fn derive_struct(ast: DataStruct, ident: Ident, generics: Generics) -> TokenStream {
    let cons = match ast.fields {
        Fields::Named(FieldsNamed { named, .. }) => gen_struct_cons(&ident, named),
        Fields::Unnamed(FieldsUnnamed { unnamed, .. }) => gen_tuple_struct_cons(&ident, unnamed),
        Fields::Unit => quote! { #ident },
    };

    gen_impl(ident, generics, cons)
}

fn gen_struct_cons<I>(ident: &Ident, fields: I) -> TokenStream
where
    I: IntoIterator<Item = Field>,
{
    // lookup each field by its name and then convert to its type using the Getable
    // impl of the fields type
    let field_initializers = fields.into_iter().map(|field| {
        let field_ty = &field.ty;
        let ident = field
            .ident
            .as_ref()
            .expect("Struct fields always have names");
        let quoted_ident = format!("{}", quote! { #ident });

        quote! {
            #ident: if let Some(val) = data.lookup_field(vm, #quoted_ident) {
                <#field_ty as ::gluon::vm::api::Getable<'__vm>>::from_value(vm, val)
            } else {
                panic!("Cannot find the field '{}'. Do the type definitions match?", #quoted_ident);
            }
        }
    });

    quote! {
        #ident {
            #(#field_initializers,)*
        }
    }
}

fn gen_tuple_struct_cons<I>(ident: &Ident, fields: I) -> TokenStream
where
    I: IntoIterator<Item = Field>,
{
    // do the lookup using the tag, because tuple structs don't have field names
    let field_initializers = fields.into_iter().enumerate().map(|(tag, field)| {
        let field_ty = &field.ty;

        quote! {
            if let Some(val) = data.get_variant(#tag) {
                <#field_ty as ::gluon::vm::api::Getable<'__vm>>::from_value(vm, val)
            } else {
                panic!("Cannot find the field with tag '{}'. Do the type definitions match?", #tag);
            }
        }
    });

    quote! {
        #ident (
            #(#field_initializers,)*
        )
    }
}

fn derive_enum(ast: DataEnum, ident: Ident, generics: Generics) -> TokenStream {
    let cons;
    {
        let variants = ast.variants
            .iter()
            .enumerate()
            .map(|(tag, variant)| gen_variant_match(&ident, tag, variant));

        // data contains the the data for each field of a variant; the variant of the passed value
        // is defined by the tag(), which is defined by order of the variants (the first variant is 0)
        cons = quote! {
            match data.tag() as usize {
                #(#variants,)*
                tag => panic!("Unexpected tag: '{}'. Do the type definitions match?", tag)
            }
        };
    }

    gen_impl(ident, generics, cons)
}

fn gen_impl(ident: Ident, generics: Generics, cons_expr: TokenStream) -> TokenStream {
    // lifetime bounds like '__vm: 'a, 'a: '__vm (which implies => 'a == '__vm)
    // writing bounds like this is a lot easier than actually replacing all lifetimes
    // with '__vm
    let lifetime_bounds = create_lifetime_bounds(&generics);

    // generate bounds like T: Getable for every type parameter
    let getable_bounds = create_getable_bounds(&generics);

    let (impl_generics, ty_generics, where_clause) = split_for_impl(&generics, &["'__vm"]);

    quote! {
        #[automatically_derived]
        #[allow(unused_attributes, unused_variables)]
        impl #impl_generics ::gluon::vm::api::Getable<'__vm> for #ident #ty_generics
        #where_clause #(#getable_bounds,)* #(#lifetime_bounds),*
        {
            fn from_value(vm: &'__vm ::gluon::vm::thread::Thread, variants: ::gluon::vm::Variants) -> Self {
                let data = match variants.as_ref() {
                    ::gluon::vm::api::ValueRef::Data(data) => data,
                    val => panic!("Unexpected value: '{:?}'. Do the type definitions match?", val),
                };

                #cons_expr
            }
        }
    }
}

fn gen_variant_match(ident: &Ident, tag: usize, variant: &Variant) -> TokenStream {
    let variant_ident = &variant.ident;

    // depending on the type of the variant we need to generate different constructors
    // for the enum
    match &variant.fields {
        Fields::Unit => quote! {
            #tag => #ident::#variant_ident
        },
        // both constructors that need to marshall values extract them by using the index
        // of the field to get the content from Data::get_variant;
        // the data variable was assigned in the function body above
        Fields::Unnamed(FieldsUnnamed { unnamed, .. }) => {
            let cons = gen_tuple_variant_cons(unnamed);

            quote! {
                #tag => #ident::#variant_ident#cons
            }
        }
        Fields::Named(FieldsNamed { named, .. }) => {
            let cons = gen_struct_variant_cons(named);

            quote! {
                #tag => #ident::#variant_ident#cons
            }
        }
    }
}

fn gen_tuple_variant_cons<'a, I>(fields: I) -> TokenStream
where
    I: IntoIterator<Item = &'a Field>,
{
    let fields = fields.into_iter().enumerate().map(|(idx, field)| {
        let field_ty = &field.ty;

        quote! {
            if let Some(val) = data.get_variant(#idx) {
                <#field_ty as ::gluon::vm::api::Getable<'__vm>>::from_value(vm, val)
            } else {
                panic!("Enum does not contain data at index '{}'. Do the type definitions match?", #idx)
            }
        }
    });

    quote!{
        (#(#fields),*)
    }
}

fn gen_struct_variant_cons<'a, I>(fields: I) -> TokenStream
where
    I: IntoIterator<Item = &'a Field>,
{
    let fields = fields.into_iter().enumerate().map(|(idx, field)| {
        let field_ty = &field.ty;
        let field_ident = field
            .ident
            .as_ref()
            .expect("Struct fields always have names");

        quote! {
            #field_ident: if let Some(val) = data.get_variant(#idx) {
                <#field_ty as ::gluon::vm::api::Getable<'__vm>>::from_value(vm, val)
            } else {
                panic!("Enum does not contain data at index '{}'. Do the type definitions match?", #idx)
            }
        }
    });

    quote!{
        {#(#fields),*}
    }
}

fn create_getable_bounds(generics: &Generics) -> Vec<TokenStream> {
    map_type_params(generics, |ty| {
        quote! {
            #ty: ::gluon::vm::api::Getable<'__vm>
        }
    })
}

fn create_lifetime_bounds(generics: &Generics) -> Vec<TokenStream> {
    map_lifetimes(generics, |lifetime| {
        quote! { #lifetime: '__vm, '__vm: #lifetime }
    })
}
