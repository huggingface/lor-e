import hashlib
import hmac


secret = "tmpsecret"
payload_body_string = '{"action":"created","comment":{"body":"test review","id":1234,"url":"https://github.com/huggingface/lor-e/5#comment-123"}, "issue":{"title":"my great contribution to the world","body":"superb work, isnt it","id":4321,"number":5,"html_url":"https://github.com/huggingface/lor-e/5", "url":"https://github.com/api/huggingface/lor-e/5"}}'


def generate_signature(payload_body, secret_token):
    hash_object = hmac.new(
        secret_token.encode("utf-8"), msg=payload_body, digestmod=hashlib.sha256
    )
    expected_signature = "sha256=" + hash_object.hexdigest()
    return expected_signature


print(generate_signature(payload_body_string.encode("utf-8"), secret))
