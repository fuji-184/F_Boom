import http from 'k6/http';
import { check } from 'k6';

export const options = {
  vus: 200,          // concurrent users
  duration: '30s',   // durasi test
};

export default function () {
  let res = http.get('http://127.0.0.1:8080/3'); // ganti dengan endpoint kamu

  // hanya cek status 200
  check(res, {
    'status is 200': (r) => r.status === 200,
  });
}
