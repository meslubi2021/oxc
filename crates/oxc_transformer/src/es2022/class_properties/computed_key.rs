//! ES2022: Class Properties
//! Transform of class property/method computed keys.

use oxc_ast::ast::*;
use oxc_syntax::symbol::SymbolFlags;
use oxc_traverse::TraverseCtx;

use super::{utils::create_assignment, ClassProperties};

impl<'a, 'ctx> ClassProperties<'a, 'ctx> {
    /// Substitute temp var for method computed key.
    /// `class C { [x()]() {} }` -> `let _x; _x = x(); class C { [_x]() {} }`
    /// This transform is only required if class has properties or a static block.
    pub(super) fn substitute_temp_var_for_method_computed_key(
        &mut self,
        method: &mut MethodDefinition<'a>,
        ctx: &mut TraverseCtx<'a>,
    ) {
        // Exit if key is not an `Expression`
        // (`PropertyKey::StaticIdentifier` or `PropertyKey::PrivateIdentifier`)
        let Some(key) = method.key.as_expression_mut() else {
            return;
        };

        // Exit if evaluating key cannot have side effects.
        // This check also results in exit for non-computed keys e.g. `class C { 'x'() {} 123() {} }`.
        if !key_needs_temp_var(key, ctx) {
            return;
        }

        // TODO(improve-on-babel): It's unnecessary to create temp vars for method keys unless:
        // 1. Properties also have computed keys.
        // 2. Some of those properties' computed keys have side effects and require temp vars.
        // 3. At least one property satisfying the above is after this method,
        //    or class contains a static block which is being transformed
        //    (static blocks are always evaluated after computed keys, regardless of order)
        let key = ctx.ast.move_expression(key);
        let temp_var = self.create_computed_key_temp_var(key, ctx);
        method.key = PropertyKey::from(temp_var);
    }

    /// Convert computed property/method key to a temp var, if a temp var is required.
    ///
    /// If no temp var is required, take ownership of key, and return it.
    ///
    /// Transformation is:
    /// * Class declaration:
    ///   `class C { [x()] = 1; }` -> `let _x; _x = x(); class C { constructor() { this[_x] = 1; } }`
    /// * Class expression:
    ///   `C = class { [x()] = 1; }` -> `let _x; C = (_x = x(), class C { constructor() { this[_x] = 1; } })`
    ///
    /// This function:
    /// * Creates the `let _x;` statement and inserts it.
    /// * Creates the `_x = x()` assignment.
    /// * Inserts assignment before class.
    /// * Returns `_x`.
    pub(super) fn create_computed_key_temp_var_if_required(
        &mut self,
        key: &mut Expression<'a>,
        ctx: &mut TraverseCtx<'a>,
    ) -> Expression<'a> {
        let key = ctx.ast.move_expression(key);
        if key_needs_temp_var(&key, ctx) {
            self.create_computed_key_temp_var(key, ctx)
        } else {
            key
        }
    }

    /// * Create `let _x;` statement and insert it.
    /// * Create `_x = x()` assignment.
    /// * Insert assignment before class.
    /// * Return `_x`.
    fn create_computed_key_temp_var(
        &mut self,
        key: Expression<'a>,
        ctx: &mut TraverseCtx<'a>,
    ) -> Expression<'a> {
        // We entered transform via `enter_expression` or `enter_statement`,
        // so `ctx.current_scope_id()` is the scope outside the class
        let parent_scope_id = ctx.current_scope_id();
        // TODO: Handle if is a class expression defined in a function's params.
        let binding =
            ctx.generate_uid_based_on_node(&key, parent_scope_id, SymbolFlags::BlockScopedVariable);

        self.ctx.var_declarations.insert_let(&binding, None, ctx);

        let assignment = create_assignment(&binding, key, ctx);
        self.insert_before.push(assignment);

        binding.create_read_expression(ctx)
    }
}

/// Check if temp var is required for `key`.
///
/// `this` does not have side effects, but in this context, it needs a temp var anyway, because `this`
/// in computed key and `this` within class constructor resolve to different `this` bindings.
/// So we need to create a temp var outside of the class to get the correct `this`.
/// `class C { [this] = 1; }`
/// -> `let _this; _this = this; class C { constructor() { this[_this] = 1; } }`
//
// TODO(improve-on-babel): Can avoid the temp var if key is for a static prop/method,
// as in that case the usage of `this` stays outside the class.
fn key_needs_temp_var(key: &Expression, ctx: &TraverseCtx) -> bool {
    match key {
        // Literals cannot have side effects.
        // e.g. `let x = 'x'; class C { [x] = 1; }` or `class C { ['x'] = 1; }`.
        Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::StringLiteral(_) => false,
        // Template literal cannot have side effects if it has no expressions.
        // If it *does* have expressions, but they're all literals, then also cannot have side effects,
        // but don't bother checking for that as it shouldn't occur in real world code.
        // Why would you write "`x${9}z`" when you can just write "`x9z`"?
        // Note: "`x${foo}`" *can* have side effects if `foo` is an object with a `toString` method.
        Expression::TemplateLiteral(lit) => !lit.expressions.is_empty(),
        // `IdentifierReference`s can have side effects if is unbound.
        //
        // If var is mutated, it also needs a temp var, because of cases like
        // `let x = 1; class { [x] = 1; [++x] = 2; }`
        // `++x` is hoisted to before class in output, so `x` in 1st key would get the wrong value
        // unless it's hoisted out too.
        //
        // TODO: Add an exec test for this odd case.
        // TODO(improve-on-babel): That case is rare.
        // Test for it in first pass over class elements, and avoid temp vars where possible.
        Expression::Identifier(ident) => {
            match ctx.symbols().get_reference(ident.reference_id()).symbol_id() {
                Some(symbol_id) => ctx.symbols().symbol_is_mutated(symbol_id),
                None => true,
            }
        }
        // Treat any other expression as possibly having side effects e.g. `foo()`.
        // TODO: Do fuller analysis to detect expressions which cannot have side effects.
        // e.g. `"x" + "y"`.
        _ => true,
    }
}