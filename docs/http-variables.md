# HTTP file variables

The graphical client and `crabcraft http` share the same variable resolver:

```http
@host = localhost:8080
@base = http://{{host}}
@token = example-token
### List items
GET {{base}}/items
Authorization: Bearer {{token}}
```

Declarations before a request apply to the entire file, including other `###` blocks.
The last declaration of a name wins. Names are case sensitive; surrounding whitespace
inside `{{ name }}` is ignored. References work in URLs, header names and values, and
request bodies. Substitution is literal; encode URL components and JSON strings yourself.
The request source and editor offsets are preserved.

A missing reference, cycle, missing closing braces or excessive expansion prevents the
request from being sent. Limits: 256 declared variables, 32 nested references and 1 MB
per expanded field and across cached variable values. Plain fields without substitutions
keep the existing HTTP client limits. No process environment, environment JSON file,
response script or OAuth helper is evaluated. Secrets stay in the file supplied by the user;
no variables are automatically loaded from the project.
