use pest::Parser;
use pest::iterators::Pair;
use tera::{Map, Context, Value, to_value};

use errors::{Result, ResultExt};
use ::context::RenderContext;

// This include forces recompiling this source file if the grammar file changes.
// Uncomment it when doing changes to the .pest file
const _GRAMMAR: &str = include_str!("content.pest");

#[derive(Parser)]
#[grammar = "content.pest"]
pub struct ContentParser;


fn replace_string_markers(input: &str) -> String {
    match input.chars().next().unwrap() {
        '"' => input.replace('"', "").to_string(),
        '\'' => input.replace('\'', "").to_string(),
        '`' => input.replace('`', "").to_string(),
        _ => unreachable!("How did you even get there"),
    }
}

fn parse_literal(pair: Pair<Rule>) -> Value {
    let mut val = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::boolean => match p.as_str() {
                "true" => val = Some(Value::Bool(true)),
                "false" => val = Some(Value::Bool(false)),
                _ => unreachable!(),
            },
            Rule::string => val = Some(Value::String(replace_string_markers(p.as_str()))),
            Rule::float => {
                val = Some(to_value(p.as_str().parse::<f64>().unwrap()).unwrap());
            }
            Rule::int => {
                val = Some(to_value(p.as_str().parse::<i64>().unwrap()).unwrap());
            }
            _ => unreachable!("Unknown literal: {:?}", p)
        };
    }

    val.unwrap()
}

/// Returns (shortcode_name, kwargs)
fn parse_shortcode_call(pair: Pair<Rule>) -> (String, Map<String, Value>) {
    let mut name = None;
    let mut args = Map::new();

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::ident => { name = Some(p.into_span().as_str().to_string()); }
            Rule::kwarg => {
                let mut arg_name = None;
                let mut arg_val = None;
                for p2 in p.into_inner() {
                    match p2.as_rule() {
                        Rule::ident => { arg_name = Some(p2.into_span().as_str().to_string()); }
                        Rule::literal => { arg_val = Some(parse_literal(p2)); }
                        Rule::array => {
                            let mut vals = vec![];
                            for p3 in p2.into_inner() {
                                match p3.as_rule() {
                                    Rule::literal => vals.push(parse_literal(p3)),
                                    _ => unreachable!("Got something other than literal in an array: {:?}", p3),
                                }
                            }
                            arg_val = Some(Value::Array(vals));
                        }
                        _ => unreachable!("Got something unexpected in a kwarg: {:?}", p2),
                    }
                }

                args.insert(arg_name.unwrap(), arg_val.unwrap());
            }
            _ => unreachable!("Got something unexpected in a shortcode: {:?}", p)
        }
    }
    (name.unwrap(), args)
}


fn render_shortcode(name: String, args: Map<String, Value>, context: &RenderContext, body: Option<&str>) -> Result<String> {
    let mut tera_context = Context::new();
    for (key, value) in args.iter() {
        tera_context.insert(key, value);
    }
    if let Some(ref b) = body {
        // Trimming right to avoid most shortcodes with bodies ending up with a HTML new line
        tera_context.insert("body", b.trim_right());
    }
    tera_context.extend(context.tera_context.clone());
    let tpl_name = format!("shortcodes/{}.html", name);

    let res = context.tera
        .render(&tpl_name, &tera_context)
        .chain_err(|| format!("Failed to render {} shortcode", name))?;

    // We trim left every single line of a shortcode to avoid the accidental
    // shortcode counted as code block because of 4 spaces left padding
    Ok(res.lines().map(|s| s.trim_left()).collect())
}

pub fn render_shortcodes(content: &str, context: &RenderContext) -> Result<String> {
    let mut res = String::with_capacity(content.len());

    let mut pairs = match ContentParser::parse(Rule::page, content) {
        Ok(p) => p,
        Err(e) => {
            let fancy_e = e.renamed_rules(|rule| {
                match *rule {
                    Rule::int => "an integer".to_string(),
                    Rule::float => "a float".to_string(),
                    Rule::string => "a string".to_string(),
                    Rule::literal => "a literal (int, float, string, bool)".to_string(),
                    Rule::array => "an array".to_string(),
                    Rule::kwarg => "a keyword argument".to_string(),
                    Rule::ident => "an identifier".to_string(),
                    Rule::inline_shortcode => "an inline shortcode".to_string(),
                    Rule::ignored_inline_shortcode => "an ignored inline shortcode".to_string(),
                    Rule::sc_body_start => "the start of a shortcode".to_string(),
                    Rule::ignored_sc_body_start => "the start of an ignored shortcode".to_string(),
                    Rule::text => "some text".to_string(),
                    _ => format!("TODO error: {:?}", rule).to_string(),
                }
            });
            bail!("{}", fancy_e);
        }
    };

    // We have at least a `page` pair
    for p in pairs.next().unwrap().into_inner() {
        match p.as_rule() {
            Rule::text | Rule::text_in_ignored_body_sc | Rule::text_in_body_sc => res.push_str(p.into_span().as_str()),
            Rule::inline_shortcode => {
                let (name, args) = parse_shortcode_call(p);
                res.push_str(&render_shortcode(name, args, context, None)?);
            }
            Rule::shortcode_with_body => {
                let mut inner = p.into_inner();
                // 3 items in inner: call, body, end
                // we don't care about the closing tag
                let (name, args) = parse_shortcode_call(inner.next().unwrap());
                let body = inner.next().unwrap().into_span().as_str();
                res.push_str(&render_shortcode(name, args, context, Some(body))?);
            }
            Rule::ignored_inline_shortcode => {
                res.push_str(
                    &p.into_span().as_str()
                        .replacen("{{/*", "{{", 1)
                        .replacen("*/}}", "}}", 1)
                );
            }
            Rule::ignored_shortcode_with_body => {
                for p2 in p.into_inner() {
                    match p2.as_rule() {
                        Rule::ignored_sc_body_start | Rule::ignored_sc_body_end => {
                            res.push_str(
                                &p2.into_span().as_str()
                                    .replacen("{%/*", "{%", 1)
                                    .replacen("*/%}", "%}", 1)
                            );
                        }
                        Rule::text_in_ignored_body_sc => res.push_str(p2.into_span().as_str()),
                        _ => unreachable!("Got something weird in an ignored shortcode: {:?}", p2),
                    }
                }
            }
            _ => unreachable!("unexpected page rule: {:?}", p.as_rule()),
        }
    }

    Ok(res)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use tera::Tera;
    use config::Config;
    use front_matter::InsertAnchor;
    use super::*;

    macro_rules! assert_lex_rule {
        ($rule: expr, $input: expr) => {
            let res = ContentParser::parse($rule, $input);
            println!("{:?}", $input);
            println!("{:#?}", res);
            if res.is_err() {
                println!("{}", res.unwrap_err());
                panic!();
            }
            assert!(res.is_ok());
            assert_eq!(res.unwrap().last().unwrap().into_span().end(), $input.len());
        };
    }

    fn render_shortcodes(code: &str, tera: &Tera) -> String {
        let config = Config::default();
        let permalinks = HashMap::new();
        let context = RenderContext::new(&tera, &config, "", &permalinks, InsertAnchor::None);
        super::render_shortcodes(code, &context).unwrap()
    }

    #[test]
    fn lex_text() {
        let inputs = vec!["Hello world", "HEllo \n world", "Hello 1 2 true false 'hey'"];
        for i in inputs {
            assert_lex_rule!(Rule::text, i);
        }
    }

    #[test]
    fn lex_inline_shortcode() {
        let inputs = vec![
            "{{ youtube() }}",
            "{{ youtube(id=1, autoplay=true, url='hey') }}",
            "{{ youtube(id=1, \nautoplay=true, url='hey') }}",
        ];
        for i in inputs {
            assert_lex_rule!(Rule::inline_shortcode, i);
        }
    }

    #[test]
    fn lex_inline_ignored_shortcode() {
        let inputs = vec![
            "{{/* youtube() */}}",
            "{{/* youtube(id=1, autoplay=true, url='hey') */}}",
            "{{/* youtube(id=1, \nautoplay=true, \nurl='hey') */}}",
        ];
        for i in inputs {
            assert_lex_rule!(Rule::ignored_inline_shortcode, i);
        }
    }

    #[test]
    fn lex_shortcode_with_body() {
        let inputs = vec![
            r#"{% youtube() %}
            Some text
            {% end %}"#,
            r#"{% youtube(id=1,
            autoplay=true, url='hey') %}
            Some text
            {% end %}"#,
        ];
        for i in inputs {
            assert_lex_rule!(Rule::shortcode_with_body, i);
        }
    }

    #[test]
    fn lex_ignored_shortcode_with_body() {
        let inputs = vec![
            r#"{%/* youtube() */%}
            Some text
            {%/* end */%}"#,
            r#"{%/* youtube(id=1,
            autoplay=true, url='hey') */%}
            Some text
            {%/* end */%}"#,
        ];
        for i in inputs {
            assert_lex_rule!(Rule::ignored_shortcode_with_body, i);
        }
    }

    #[test]
    fn lex_page() {
        let inputs = vec![
            "Some text and a shortcode `{{/* youtube() */}}`",
            "{{ youtube(id=1, autoplay=true, url='hey') }}",
            "{{ youtube(id=1, \nautoplay=true, url='hey') }} that's it",
            r#"
            This is a test
            {% hello() %}
            Body {{ var }}
            {% end %}
            "#
        ];
        for i in inputs {
            assert_lex_rule!(Rule::page, i);
        }
    }

    #[test]
    fn does_nothing_with_no_shortcodes() {
        let res = render_shortcodes("Hello World", &Tera::default());
        assert_eq!(res, "Hello World");
    }

    #[test]
    fn can_unignore_inline_shortcode() {
        let res = render_shortcodes("Hello World {{/* youtube() */}}", &Tera::default());
        assert_eq!(res, "Hello World {{ youtube() }}");
    }

    #[test]
    fn can_unignore_shortcode_with_body() {
        let res = render_shortcodes(r#"
Hello World
{%/* youtube() */%}Some body {{ hello() }}{%/* end */%}"#, &Tera::default());
        assert_eq!(res, "\nHello World\n{% youtube() %}Some body {{ hello() }}{% end %}");
    }

    #[test]
    fn can_parse_shortcode_arguments() {
        let inputs = vec![
            ("{{ youtube() }}", "youtube", Map::new()),
            (
                "{{ youtube(id=1, autoplay=true, hello='salut', float=1.2) }}",
                "youtube",
                {
                    let mut m = Map::new();
                    m.insert("id".to_string(), to_value(1).unwrap());
                    m.insert("autoplay".to_string(), to_value(true).unwrap());
                    m.insert("hello".to_string(), to_value("salut").unwrap());
                    m.insert("float".to_string(), to_value(1.2).unwrap());
                    m
                }
            ),
            (
                "{{ gallery(photos=['something', 'else'], fullscreen=true) }}",
                "gallery",
                {
                    let mut m = Map::new();
                    m.insert("photos".to_string(), to_value(["something", "else"]).unwrap());
                    m.insert("fullscreen".to_string(), to_value(true).unwrap());
                    m
                }
            ),
        ];

        for (i, n, a) in inputs {
            let mut res = ContentParser::parse(Rule::inline_shortcode, i).unwrap();
            let (name, args) = parse_shortcode_call(res.next().unwrap());
            assert_eq!(name, n);
            assert_eq!(args, a);
        }
    }

    #[test]
    fn can_render_inline_shortcodes() {
        let mut tera = Tera::default();
        tera.add_raw_template("shortcodes/youtube.html", "Hello {{id}}").unwrap();
        let res = render_shortcodes("Inline {{ youtube(id=1) }}.", &tera);
        assert_eq!(res, "Inline Hello 1.");
    }

    #[test]
    fn can_render_shortcodes_with_body() {
        let mut tera = Tera::default();
        tera.add_raw_template("shortcodes/youtube.html", "{{body}}").unwrap();
        let res = render_shortcodes("Body\n {% youtube() %}Hey!{% end %}", &tera);
        assert_eq!(res, "Body\n Hey!");
    }
}
