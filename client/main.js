import './style.css'
import FoamTree from '@carrotsearch/foamtree';
import groups from './groups.json'

const appElem = document.getElementById('app');
console.log(appElem)
const foamtree = new FoamTree({
  id: "app",
  layout: 'squarified',
  stacking: 'flattened',
  dataObject: {
    groups
  },
});

window.addEventListener("resize", (() => {
  let timeout;
  return () => {
    window.clearTimeout(timeout);
    timeout = window.setTimeout(foamtree.resize, 300);
  };
})());