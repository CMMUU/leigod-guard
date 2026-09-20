import {copyFileSync,mkdirSync} from 'node:fs';
mkdirSync('static/vendor',{recursive:true});
for (const [from,to] of [
 ['node_modules/@tabler/core/dist/css/tabler.min.css','tabler.min.css'],
 ['node_modules/echarts/dist/echarts.min.js','echarts.min.js'],
 ['node_modules/echarts/LICENSE','echarts-LICENSE'],
 ['node_modules/echarts/NOTICE','echarts-NOTICE'],
]) copyFileSync(from,'static/vendor/'+to);
