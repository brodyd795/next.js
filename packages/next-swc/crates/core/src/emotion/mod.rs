use std::path::Path;
use std::sync::Arc;

use fxhash::FxHashMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use swc_common::{FileName, SourceFile, DUMMY_SP};
use swc_ecmascript::ast::{
    ExprOrSpread, Ident, KeyValueProp, Lit, ObjectLit, Prop, PropName, PropOrSpread,
};
use swc_ecmascript::{
    ast::{Callee, Expr, ImportDecl, ImportSpecifier},
    visit::{swc_ecma_ast::CallExpr, Fold},
};

use self::global_parent_cache::RootPathInfo;

mod global_parent_cache;
mod hash;

static EMOTION_OFFICIAL_LIBRARIES: Lazy<Vec<EmotionModuleConfig>> = Lazy::new(|| {
    vec![
        EmotionModuleConfig {
            module_name: "@emotion/styled".to_owned(),
            exported_names: vec!["styled".to_owned()],
            has_default_export: Some(true),
            ..Default::default()
        },
        EmotionModuleConfig {
            module_name: "@emotion/react".to_owned(),
            exported_names: vec!["css".to_owned()],
            ..Default::default()
        },
    ]
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionOptions {
    enabled: Option<bool>,
    sourcemap: Option<bool>,
    auto_label: Option<bool>,
    label_format: Option<String>,
    auto_inject: Option<bool>,
    custom_modules: Option<Vec<EmotionModuleConfig>>,
    jsx_factory: Option<String>,
    jsx_import_source: Option<String>,
}

impl Default for EmotionOptions {
    fn default() -> Self {
        EmotionOptions {
            enabled: Some(false),
            sourcemap: Some(true),
            auto_label: Some(true),
            label_format: Some("[local]".to_owned()),
            auto_inject: Some(true),
            custom_modules: None,
            jsx_import_source: Some("@emotion/react".to_owned()),
            jsx_factory: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmotionModuleConfig {
    module_name: String,
    exported_names: Vec<String>,
    include_sub_path: Option<bool>,
    has_default_export: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ImportType {
    Named,
    Namespace,
    Default,
}

impl Default for ImportType {
    fn default() -> Self {
        ImportType::Named
    }
}

#[derive(Debug)]
struct PackageMeta {
    _type: ImportType,
}

#[derive(Debug)]
pub struct EmotionTransformer {
    pub options: EmotionOptions,
    source_file: Arc<SourceFile>,
    _react_jsx_runtime: bool,
    _es_module_interop: bool,
    custom_modules: Vec<EmotionModuleConfig>,
    import_packages: FxHashMap<String, PackageMeta>,
    emotion_target_class_name_count: usize,
}

impl EmotionTransformer {
    pub fn new(
        options: EmotionOptions,
        source_file: Arc<SourceFile>,
        react_jsx_runtime: bool,
        es_module_interop: bool,
    ) -> Self {
        EmotionTransformer {
            custom_modules: options.custom_modules.clone().unwrap_or_default(),
            options,
            source_file,
            _react_jsx_runtime: react_jsx_runtime,
            import_packages: FxHashMap::default(),
            _es_module_interop: es_module_interop,
            emotion_target_class_name_count: 0,
        }
    }

    // Find the imported name from modules
    // These import statements are supported:
    //    import styled from '@emotion/styled'
    //    import { default as whateverStyled } from '@emotion/styled'
    //    import * as styled from '@emotion/styled'  // with `no_interop: true`
    //    import { css } from '@emotion/react'
    //    import emotionCss from '@emotion/react'
    //    import * as emotionCss from '@emotion/react' // with `no_interop: true`
    fn generate_import_info(&mut self, expr: &ImportDecl) {
        for c in EMOTION_OFFICIAL_LIBRARIES
            .iter()
            .chain(self.custom_modules.iter())
        {
            if expr.src.value == c.module_name {
                for specifier in expr.specifiers.iter() {
                    match specifier {
                        ImportSpecifier::Named(named) => {
                            for export_name in c.exported_names.iter() {
                                if named.local.as_ref() == export_name {
                                    self.import_packages.insert(
                                        named.local.to_string(),
                                        PackageMeta {
                                            _type: ImportType::Named,
                                        },
                                    );
                                }
                            }
                        }
                        ImportSpecifier::Default(default) => {
                            if c.has_default_export.unwrap_or(false) {
                                self.import_packages.insert(
                                    default.local.to_string(),
                                    PackageMeta {
                                        _type: ImportType::Default,
                                    },
                                );
                            }
                        }
                        ImportSpecifier::Namespace(namespace) => {
                            self.import_packages.insert(
                                namespace.local.to_string(),
                                PackageMeta {
                                    _type: ImportType::Namespace,
                                },
                            );
                        }
                    }
                }
            }
        }
    }
}

impl Fold for EmotionTransformer {
    // Collect import modules that indicator if this file need to be transformed
    fn fold_import_decl(&mut self, expr: ImportDecl) -> ImportDecl {
        if expr.type_only {
            return expr;
        }
        self.generate_import_info(&expr);
        expr
    }

    fn fold_call_expr(&mut self, mut expr: CallExpr) -> CallExpr {
        // If no package that we care about is imported, skip the following
        // transformation logic.
        if self.import_packages.is_empty() {
            return expr;
        }
        if let Callee::Expr(e) = &mut expr.callee {
            match e.as_ref() {
                // css({})
                Expr::Ident(i) => {
                    if self.import_packages.get(i.as_ref()).is_some() && !expr.args.is_empty() {
                        if let FileName::Real(filename) = &self.source_file.name {
                            let root_info = find_root(filename).unwrap_or_else(|| {
                                RootPathInfo::new("".to_owned(), filename.to_path_buf())
                            });
                            let final_path = if &root_info.root_path == filename {
                                "root"
                            } else {
                                root_info
                                    .root_path
                                    .to_str()
                                    .and_then(|root| {
                                        filename
                                            .to_str()
                                            .map(|filename| filename.trim_start_matches(root))
                                    })
                                    .unwrap_or_else(|| self.source_file.src.as_str())
                            };
                            let stable_class_name = format!(
                                "e{}{}",
                                hash::murmurhash2(
                                    format!("{}{}", &root_info.package_name, final_path).as_bytes()
                                ),
                                self.emotion_target_class_name_count
                            );
                            self.emotion_target_class_name_count += 1;
                            let target_assignment =
                                PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                                    key: PropName::Ident(Ident::new("target".into(), DUMMY_SP)),
                                    value: Box::new(Expr::Lit(Lit::Str(stable_class_name.into()))),
                                })));
                            match expr.args.len() {
                                1 => {
                                    expr.args.push(ExprOrSpread {
                                        spread: None,
                                        expr: Box::new(Expr::Object(ObjectLit {
                                            span: DUMMY_SP,
                                            props: vec![target_assignment],
                                        })),
                                    });
                                }
                                2 => {
                                    if let Expr::Object(lit) = expr.args[1].expr.as_mut() {
                                        lit.props.push(target_assignment);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                // styled('div')({})
                Expr::Call(_c) => {}
                // styled.div({})
                // customEmotionReact.css({})
                Expr::Member(_m) => {}
                _ => {}
            }
        }
        expr
    }
}

fn find_root(p: &Path) -> Option<RootPathInfo> {
    if let Some(parent) = p.parent() {
        let parent = parent.to_path_buf();
        if let Some(p) = global_parent_cache::GLOBAL_PARENT_CACHE.get(&parent) {
            return Some(p);
        }
        if parent.exists() {
            if parent.join("package.json").exists() {
                return Some(
                    global_parent_cache::GLOBAL_PARENT_CACHE.insert(parent.clone(), parent),
                );
            } else {
                return find_root(&parent);
            }
        }
    }
    None
}
