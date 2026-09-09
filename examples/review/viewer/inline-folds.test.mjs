import {test} from 'node:test';
import assert from 'node:assert/strict';
import {foldLine} from './inline-folds.mjs';
const range=(a,b,c=a,d=b)=>({start:{line:a,byte_column:b},end:{line:c,byte_column:d}});
const render=(source,row,folds,closed)=>foldLine(Buffer.byteLength(source),row,folds,new Set(closed)).map(p=>p.text?Buffer.from(source).subarray(...p.text).toString():p.collapsed?'…':'').join('');
test('inline fold preserves prefix, suffix, unicode and expansion',()=>{
 const src='call("☕", value);';
 const folds=[{id:0,range:range(0,5,0,17)}];
 assert.equal(render(src,0,folds,[0]),'call(…);');
 assert.equal(render(src,0,folds,[]),src);
});
test('nested fold state survives folding its parent',()=>{
 const src='f(g(a), b)';
 const folds=[{id:0,range:range(0,2,0,9)},{id:1,range:range(0,4,0,5)}];
 assert.equal(render(src,0,folds,[1]),'f(g(…), b)');
 assert.equal(render(src,0,folds,[0,1]),'f(…)');
 assert.equal(render(src,0,folds,[1]),'f(g(…), b)');
});
test('multiline partial range retains both boundary fragments',()=>{
 const folds=[{id:0,range:range(0,3,2,4)}];
 assert.equal(render('abc hidden',0,folds,[0]),'abc…');
 assert.equal(render('all hidden',1,folds,[0]),'');
 assert.equal(render('hide suffix',2,folds,[0]),' suffix');
});
