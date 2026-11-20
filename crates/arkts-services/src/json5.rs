//! JSON5 support for ArkTS configuration files (`module.json5`, `app.json5`).
//!
//! OXC's expression parser accepts the JSON5 superset (comments, single quotes,
//! unquoted keys and trailing commas) while keeping spans in the original source,
//! so resource references and paths can be mapped without a second parser.
use crate::{contains, offset_at, ResourceCall};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrayExpressionElement, Expression, ObjectExpression, ObjectPropertyKind, StringLiteral,
};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_span::SourceType;
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct Json5Document {
    pub text: String,
    /// Every string literal, including resource references and permission names.
    pub strings: Vec<ResourceCall>,
    /// Strings under `requestPermissions[].name`.
    pub permissions: Vec<ResourceCall>,
}

impl Json5Document {
    pub fn parse(text: String) -> Self {
        let allocator = Allocator::default();
        let Ok(expression) =
            Parser::new(&allocator, &text, SourceType::default()).parse_expression()
        else {
            return Self {
                text,
                ..Self::default()
            };
        };
        let mut visitor = Strings::default();
        visitor.visit_expression(&expression);
        Self {
            text,
            strings: visitor.strings,
            permissions: visitor.permissions,
        }
    }

    pub fn string_at(&self, position: &Value) -> Option<&ResourceCall> {
        let offset = offset_at(&self.text, position)?;
        self.strings.iter().find(|call| contains(call.span, offset))
    }

    pub fn permission_at(&self, position: &Value) -> Option<&ResourceCall> {
        let offset = offset_at(&self.text, position)?;
        self.permissions
            .iter()
            .find(|call| contains(call.span, offset))
    }

    /// `$type:name` references in the same order they appear in the file.
    pub fn references(&self) -> impl Iterator<Item = &ResourceCall> {
        self.strings
            .iter()
            .filter(|call| call.value.starts_with('$') && call.value.contains(':'))
    }
}

#[derive(Default)]
struct Strings {
    strings: Vec<ResourceCall>,
    permissions: Vec<ResourceCall>,
}

impl<'a> Visit<'a> for Strings {
    fn visit_string_literal(&mut self, value: &StringLiteral<'a>) {
        self.strings.push(ResourceCall {
            value: value.value.as_str().unwrap_or_default().to_string(),
            span: value.span,
        });
    }

    fn visit_object_expression(&mut self, object: &ObjectExpression<'a>) {
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                continue;
            };
            if !property
                .key
                .static_name()
                .is_some_and(|name| name.as_str() == Some("requestPermissions"))
            {
                continue;
            }
            let Expression::ArrayExpression(array) = &property.value else {
                continue;
            };
            for element in &array.elements {
                let ArrayExpressionElement::ObjectExpression(object) = element else {
                    continue;
                };
                for property in &object.properties {
                    let ObjectPropertyKind::ObjectProperty(property) = property else {
                        continue;
                    };
                    if !property
                        .key
                        .static_name()
                        .is_some_and(|name| name.as_str() == Some("name"))
                    {
                        continue;
                    }
                    let Expression::StringLiteral(value) = &property.value else {
                        continue;
                    };
                    self.permissions.push(ResourceCall {
                        value: value.value.as_str().unwrap_or_default().to_string(),
                        span: value.span,
                    });
                }
            }
        }
        walk::walk_object_expression(self, object);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json5_and_keeps_original_spans() {
        let text = r#"{
  // JSON5 allows comments, single quotes, unquoted keys and trailing commas.
  module: {
    name: 'entry',
    icon: "$media:icon",
    pages: '$profile:main_pages',
    requestPermissions: [
      { name: 'ohos.permission.INTERNET' },
    ],
  },
}"#;
        let document = Json5Document::parse(text.into());
        let icon = document
            .strings
            .iter()
            .find(|call| call.value == "$media:icon")
            .unwrap();
        assert_eq!(
            &text[icon.span.start as usize..icon.span.end as usize],
            "\"$media:icon\""
        );
        assert_eq!(document.permissions.len(), 1);
        assert_eq!(document.permissions[0].value, "ohos.permission.INTERNET");
        let references: Vec<_> = document
            .references()
            .map(|call| call.value.as_str())
            .collect();
        assert_eq!(references, ["$media:icon", "$profile:main_pages"]);
    }
}
