"""Enforce the approved fake-remote fixture boundary before executing test code."""
import ast

def validate_source(source):
 tree=ast.parse(source)
 for node in ast.walk(tree):
  if isinstance(node,ast.Constant) and isinstance(node.value,str):
   value=node.value
   if value=='--bare' or 'insteadOf' in value or value.startswith('/tmp/opencode'):
    raise ValueError('Queue fixture requires in-process FakeRemote; bare Git, URL rewriting and external socket aliases are forbidden')
  if isinstance(node,ast.Call) and isinstance(node.func,ast.Attribute) and node.func.attr=='__new__':
   target=node.func.value
   if isinstance(target,ast.Attribute) and target.attr=='Queue':
    raise ValueError('Queue fixture must use the existing Queue constructor')
 return True
